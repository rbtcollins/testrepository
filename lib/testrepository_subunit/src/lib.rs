//! Helpers for working with subunit streams.
//!
//! Some or all could be moved to subunit eventually.

// Copyright (c) 2009,2024 Testrepository Contributors
//
// Licensed under either the Apache License, Version 2.0 or the BSD 3-clause
// license at the users choice. A copy of both licenses are available in the
// project source as Apache-2.0 and BSD. You may not use this file except in
// compliance with one of these two licences.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under these licenses is distributed on an "AS IS" BASIS, WITHOUT
// WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.  See the
// license you chose for the specific language governing permissions and
// limitations under that license.

use std::{
    collections::{BTreeMap, HashSet},
    future::ready,
    mem,
};

use async_stream::try_stream;
use chrono::{DateTime, Utc};
use futures::stream;
use subunit::{
    types::{
        event::{self, Event},
        file::File,
        stream::{ScannedItem, UTF8VariableLength},
        teststatus::TestStatus,
    },
    v1::{Event as V1Event, Part},
};
use thiserror::Error;
use tokio_stream::{Stream, StreamExt};

trait V2Ext {
    /// Is this the last event for a test? This is true if the status is not
    /// InProgress or Undefined. It wouldn't be possible to infer an end event
    /// for a test if the generator attached multiple files, as each file is at
    /// least a single v2 event.
    ///
    /// If a stream does include such a pattern, the v1 converted stream will
    /// still have all the content, but multiple reports of the test will be
    /// present.
    fn is_final(&self) -> bool;
}

impl V2Ext for Event {
    fn is_final(&self) -> bool {
        ![TestStatus::InProgress, TestStatus::Undefined].contains(&self.status)
    }
}

trait FileExt {
    /// Can't use Into because of the orphan rule.
    fn as_part(&self) -> Option<Part>;
}

impl FileExt for File {
    fn as_part(&self) -> Option<Part> {
        self.file.as_ref().map(|(name, bytes)| Part {
            content_type: self
                .mime_type
                .as_deref()
                .unwrap_or("application/octet-stream")
                .to_string(),
            name: name.clone(),
            bytes: bytes.clone(),
        })
    }
}

/// map all the events for a single test to a vector of v1 events.
fn convert_test_events(events: Vec<Event>) -> Vec<Result<V1Event, ConversionError>> {
    let mut v1_events = Vec::new();
    if events.is_empty() || events.first().unwrap().test_id.is_none() {
        return v1_events;
    }
    let last = events.last().unwrap();
    let status = last.status;
    let first = events.first().unwrap();
    let test_id = first.test_id.as_ref().unwrap().clone();
    let mut tags: Vec<String> = events
        .iter()
        .flat_map(|e| e.tags.iter().flatten())
        .cloned()
        .collect();
    tags.sort();
    tags.dedup();

    let events = events
        .iter()
        .filter(|e| e.status != TestStatus::Undefined)
        .collect::<Vec<_>>();

    // we expect 1 to many events; typical case should be 1 InProgress events to
    // establish the start time, then more with file data followed by one final
    // event wrapping the test up.
    if let Some(timestamp) = first.timestamp.clone() {
        v1_events.push(
            DateTime::<Utc>::try_from(timestamp)
                .map(V1Event::Time)
                .map_err(ConversionError::from),
        );
    }
    if !tags.is_empty() {
        v1_events.push(Ok(V1Event::Tags(tags.clone(), vec![])));
    }
    v1_events.push(Ok(V1Event::TestStart(test_id.clone())));

    let mut parts = BTreeMap::new();
    let mut end_time = &first.timestamp;

    for event in events {
        if event.timestamp.is_some() {
            end_time = &event.timestamp;
        }
        if event.file.file.is_some() {
            // TODO? reduce allocations
            if let Some(p) = event.file.as_part() {
                let entry = parts.entry(p.name.clone());
                let buffered_part = entry.or_insert_with(|| Part {
                    content_type: p.content_type,
                    name: p.name,
                    bytes: vec![],
                });
                buffered_part.bytes.extend_from_slice(&p.bytes);
            }
        }
    }
    let parts = parts.into_values().collect();
    if let Some(timestamp) = end_time {
        v1_events.push(
            DateTime::<Utc>::try_from(timestamp.clone())
                .map(V1Event::Time)
                .map_err(ConversionError::from),
        );
    }

    v1_events.push(Ok(match status {
        TestStatus::Success => V1Event::TestSuccess(test_id, parts),
        TestStatus::Undefined => unreachable!(),
        TestStatus::Enumeration => {
            // weird - discovery streams don't make sense to convert...  but lets handle it. we have an open test and need to close it
            V1Event::TestSuccess(test_id, parts)
        }
        TestStatus::InProgress => {
            // test never finished, treat it as an error
            // todo: perhaps add an explanation?
            V1Event::TestError(test_id, parts)
        }
        TestStatus::UnexpectedSuccess => V1Event::TestUnexpectedSuccess(test_id, parts),
        TestStatus::Skipped => V1Event::TestSkip(test_id, parts),
        TestStatus::Failed => V1Event::TestFailure(test_id, parts),
        TestStatus::ExpectedFailure => V1Event::TestExpectedFailure(test_id, parts),
    }));
    // NB: this sets tags globally to affect StartTest, and brackets every test.
    // A smaller stream would be possible if runs of an applicable tag were
    // detected and the redundant operations eliminated.
    if !tags.is_empty() {
        v1_events.push(Ok(V1Event::Tags(vec![], tags)));
    }
    v1_events
}

/// map a Subunit v2 stream to a subunit v1 stream.
///
/// This will buffer events for a test until a 'final' status is reached. This
/// means that large file attachments may cause memory allocation errors.
pub fn convert_v2_to_v1<S, E>(from_: S) -> impl Stream<Item = Result<V1Event, ConversionError>>
where
    E: std::error::Error,
    S: Stream<Item = Result<Event, E>>,
    ConversionError: From<E>,
{
    let from_ = from_.map(Option::Some).chain(stream::once(async { None }));

    let converted = futures::StreamExt::scan(from_, BTreeMap::new(), |buffered_events, event| {
        tracing::trace!("event: {:?}", event);
        match event {
            Some(Ok(event)) => {
                if event.test_id.is_none() {
                    // Non-test events are discarded (for now?).
                    return ready(Some(vec![]));
                }
                let test_id = event.test_id.as_ref().unwrap();
                // In the pathological case, the first event might cause the entire
                // stream to be buffered and then flush out at the end. Instead, we
                // buffer everything and flush when the last event for a test is
                // received
                if event.is_final() {
                    let mut buffered: Vec<Event> =
                        buffered_events.remove(test_id).unwrap_or_default();
                    buffered.push(event);
                    let v1_events = convert_test_events(buffered);
                    ready(Some(v1_events))
                } else {
                    let entry = buffered_events.entry(test_id.to_string());

                    entry.or_default().push(event);
                    ready(Some(vec![])) // nothing to emit
                }
            }
            Some(Err(e)) => ready(Some(vec![Err(e.into())])),
            None => {
                // End of input stream, flush all buffered events in arbitrary order.
                let v1_events = mem::take(buffered_events)
                    .into_iter()
                    .flat_map(|(_, events)| convert_test_events(events))
                    .collect::<Vec<_>>();
                ready(Some(v1_events))
            }
        }
    })
    .map(stream::iter);
    futures::StreamExt::flatten(converted)
}

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Invalid UTF8 sequence in bytes '{:?}'", _0)]
    InvalidUTF8Sequence(UTF8VariableLength),
    // TODO: consider the size of ScannedItem : it includes Event at 168 bytes.
    #[error("Unknown scanned item '{:?}'", _0)]
    Unknown(ScannedItem),
    #[error("subunit {}", _0)]
    Subunit(#[from] subunit::Error),
    #[error("Generic error")]
    Generic(#[from] Box<dyn std::error::Error>),
}

/// Converts scanned items to errors if they are not events.
pub fn only_events<S: Stream<Item = Result<ScannedItem, Box<dyn std::error::Error>>>>(
    from_: S,
) -> impl Stream<Item = Result<Event, ConversionError>> {
    try_stream! {
        for await event in from_ {
            let event = event?;
            match event {
                ScannedItem::UTF8chars(b) => Err(ConversionError::InvalidUTF8Sequence(b) )? ,
                ScannedItem::Event(event) => yield event,
                i@ ScannedItem::Unknown(_, _) => Err(ConversionError::Unknown(i))?,
            }
        }
    }
}

#[derive(Debug, Default)]
struct V1To2State {
    in_test: bool,
    global_tags: HashSet<String>,
    test_tags: HashSet<String>,
    datetime: Option<DateTime<Utc>>,
}

impl V1To2State {
    fn apply_tags(&mut self, added: Vec<String>, removed: Vec<String>) {
        let tags = if self.in_test {
            &mut self.test_tags
        } else {
            &mut self.global_tags
        };
        for tag in removed {
            tags.remove(&tag);
        }
        for tag in added {
            tags.insert(tag);
        }
    }

    fn start_test(&mut self) {
        self.in_test = true;
    }

    fn finish_test(
        &mut self,
        status: TestStatus,
        test_id: &str,
        details: Vec<Part>,
    ) -> Vec<Result<event::Event, ConversionError>> {
        let mut result = vec![];

        for detail in details {
            let chunk_size = 4194303 - 50 - detail.name.len() - test_id.len();
            result.extend(
                // NB: A helper in subunit would be good here, because calculating the maxmium size of any one chunk is .. complicated.
                detail
                    .bytes
                    .chunks(chunk_size)
                    .map(Option::Some)
                    .chain(std::iter::once(None))
                    .map(|chunk| {
                        // NB this generates a single event for EOF, which is not ideal.
                        self.make_event(TestStatus::InProgress, test_id)
                            .map(|mut builder| {
                                builder = builder.mime_type(&detail.content_type);
                                match chunk {
                                    Some(chunk) => {
                                        builder = builder.file_content(&detail.name, chunk)
                                    }
                                    None => {
                                        builder = builder.file_content(&detail.name, &[]);
                                        builder = builder.end_of_file()
                                    }
                                }
                                builder.build()
                            })
                    }),
            );
        }
        result.push(
            self.make_event(status, test_id)
                .map(event::EventBuilder::build),
        );

        self.in_test = false;
        self.test_tags.clear();
        result
    }

    #[allow(clippy::result_large_err)]
    fn make_event(
        &self,
        status: TestStatus,
        test_id: &str,
    ) -> Result<event::EventBuilder, ConversionError> {
        let mut builder = Event::new(status).test_id(test_id);
        let mut tags = self
            .test_tags
            .iter()
            .chain(self.global_tags.iter())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        for tag in tags.iter() {
            builder = builder.tag(tag);
        }
        if let Some(datetime) = self.datetime {
            builder = builder.datetime(datetime)?;
        }
        Ok(builder)
    }
}

/// map a Subunit v1 stream to a subunit v2 stream. tags within a v1 test are
/// not applied to events for the same test that are output before the tags
/// instruction. In particular, the TestStart event is not tagged. This permits
/// not buffering open tests that may not terminate within the stream.
pub fn convert_v1_to_v2<S, E>(from_: S) -> impl Stream<Item = Result<Event, ConversionError>>
where
    E: std::error::Error,
    S: Stream<Item = Result<V1Event, E>>,
    ConversionError: From<E>,
{
    let converted = futures::StreamExt::scan(from_, V1To2State::default(), |state, event| {
        tracing::trace!("event: {:?}", event);
        match event {
            Ok(event) => {
                let mut events = vec![];
                match event {
                    V1Event::TestStart(test_id) => {
                        state.start_test();
                        events.push(
                            state
                                .make_event(TestStatus::InProgress, &test_id)
                                .map(event::EventBuilder::build),
                        );
                    }
                    V1Event::TestSuccess(test_id, details) => {
                        events.extend(state.finish_test(TestStatus::Success, &test_id, details));
                    }
                    V1Event::TestFailure(test_id, details) => {
                        events.extend(state.finish_test(TestStatus::Failed, &test_id, details));
                    }
                    V1Event::TestError(test_id, details) => {
                        events.extend(state.finish_test(TestStatus::Failed, &test_id, details));
                    }
                    V1Event::TestSkip(test_id, details) => {
                        events.extend(state.finish_test(TestStatus::Skipped, &test_id, details));
                    }
                    V1Event::TestExpectedFailure(test_id, details) => {
                        events.extend(state.finish_test(
                            TestStatus::ExpectedFailure,
                            &test_id,
                            details,
                        ));
                    }
                    V1Event::TestUnexpectedSuccess(test_id, details) => {
                        events.extend(state.finish_test(
                            TestStatus::UnexpectedSuccess,
                            &test_id,
                            details,
                        ));
                    }
                    // Progress events are not supported in v2
                    V1Event::ProgressPush
                    | V1Event::ProgressPop
                    | V1Event::ProgressSet(_)
                    | V1Event::ProgressCurrent(_) => (),
                    // Non-protocol content in the stream ignored
                    V1Event::Text(_text) => (),
                    V1Event::Bytes(_bytes) => (),
                    V1Event::Tags(added, removed) => {
                        state.apply_tags(added, removed);
                    }
                    V1Event::Time(datetime) => state.datetime = Some(datetime),
                    V1Event::EndOfStream => (),
                }
                ready(Some(events))
            }
            Err(e) => ready(Some(vec![Err(e.into())])),
        }
    })
    .map(stream::iter);
    futures::StreamExt::flatten(converted)
}

#[cfg(test)]
mod tests {

    use chrono::Timelike;
    use futures::TryStreamExt;
    use subunit::{
        io::r#async::{self, WriteIntoAsync},
        serialize::Serializable as _,
        types::{event::Event, teststatus::TestStatus},
        v1::{parse, Event as V1Event},
    };
    use tracing_test::traced_test;

    use crate::{convert_v1_to_v2, convert_v2_to_v1, only_events};

    #[tokio::test]
    #[traced_test]
    async fn test_convert_v2_to_v1_smoke() {
        // Construct a buffer containing a simple v2 stream
        let t1 = chrono::Utc::now();
        let t2 = t1 + chrono::Duration::seconds(1);

        let events = vec![
            // Discarded - no test id
            Event::new(TestStatus::Success).build(),
            // Start of test foo at t1
            Event::new(TestStatus::InProgress)
                .test_id("foo")
                .tag("bar")
                .tag("foo")
                .datetime(t1)
                .unwrap()
                .build(),
            // Test bar interleaved with test foo, with a different tag
            Event::new(TestStatus::InProgress)
                .test_id("bar")
                .tag("baz")
                .build(),
            // Complete test foo
            Event::new(TestStatus::Success)
                .test_id("foo")
                .datetime(t2)
                .unwrap()
                .build(),
            // Test with no tags
            Event::new(TestStatus::Success).test_id("baz").build(),
            // Test bar is not completed and included by the flush at the end of
            // the stream.
        ];

        let mut buf = Vec::new();
        for event in events {
            event.serialize(&mut buf).unwrap();
        }
        assert_ne!(buf.len(), 0);

        // Use the standard subunit deserializer to read the stream
        let stream = r#async::iter_stream(&buf[..]);

        // Discard the noise
        let stream = only_events(stream);

        let v1_events = convert_v2_to_v1(stream)
            .try_collect::<Vec<V1Event>>()
            .await
            .unwrap();

        assert_eq!(
            &[
                V1Event::Time(t1),
                V1Event::Tags(vec!["bar".into(), "foo".into()], vec![]),
                V1Event::TestStart("foo".into()),
                V1Event::Time(t2),
                V1Event::TestSuccess("foo".into(), vec![]),
                V1Event::Tags(vec![], vec!["bar".into(), "foo".into()]),
                V1Event::TestStart("baz".into()),
                V1Event::TestSuccess("baz".into(), vec![]),
                V1Event::Tags(vec!["baz".into()], vec![]),
                V1Event::TestStart("bar".into()),
                V1Event::TestError("bar".into(), vec![]),
                V1Event::Tags(vec![], vec!["baz".into()]),
            ] as &[V1Event],
            &v1_events
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_convert_v1_to_v2_smoke() {
        // Construct a buffer containing a simple v1 stream
        // v1 doesn't support subsecond resolution
        let t1 = chrono::Utc::now().with_nanosecond(0).unwrap();
        let t2 = t1 + chrono::Duration::seconds(1);

        let v1_events = vec![
            V1Event::Time(t1),
            V1Event::Tags(vec!["bar".into(), "foo".into()], vec![]),
            V1Event::TestStart("foo".into()),
            V1Event::Time(t2),
            V1Event::TestSuccess("foo".into(), vec![]),
            V1Event::Tags(vec![], vec!["bar".into(), "foo".into()]),
            V1Event::TestStart("baz".into()),
            V1Event::TestSuccess("baz".into(), vec![]),
            V1Event::Tags(vec!["baz".into()], vec![]),
            V1Event::TestStart("bar".into()),
            V1Event::TestError("bar".into(), vec![]),
            V1Event::Tags(vec![], vec!["baz".into()]),
        ];

        let mut buf = Vec::new();
        for event in v1_events {
            event.write_into(&mut buf).await.unwrap();
        }
        assert_ne!(buf.len(), 0);
        let mut buf = &buf[..];

        // Use the standard subunit v1 deserializer to read the stream
        let stream = parse(&mut buf);
        // let events = stream
        //     .collect::<Result<Vec<_>, crate::Error>>()
        //     .await
        //     .unwrap();
        let v2_events = convert_v1_to_v2(stream)
            .try_collect::<Vec<Event>>()
            .await
            .unwrap();

        assert_eq!(
            &[
                // Start of test foo at t1
                Event::new(TestStatus::InProgress)
                    .test_id("foo")
                    .tag("bar")
                    .tag("foo")
                    .datetime(t1)
                    .unwrap()
                    .build(),
                // Complete test foo
                Event::new(TestStatus::Success)
                    .test_id("foo")
                    .tag("bar")
                    .tag("foo")
                    .datetime(t2)
                    .unwrap()
                    .build(),
                // Test with no tags
                Event::new(TestStatus::InProgress)
                    .test_id("baz")
                    .datetime(t2)
                    .unwrap()
                    .build(),
                Event::new(TestStatus::Success)
                    .test_id("baz")
                    .datetime(t2)
                    .unwrap()
                    .build(),
                // Test bar on the v2->1 pass was not completed and included by
                // the flush at the end of the stream, on v1->v2 it is just a regular test
                Event::new(TestStatus::InProgress)
                    .test_id("bar")
                    .tag("baz")
                    .datetime(t2)
                    .unwrap()
                    .build(),
                Event::new(TestStatus::Failed)
                    .test_id("bar")
                    .tag("baz")
                    .datetime(t2)
                    .unwrap()
                    .build(),
            ] as &[Event],
            &v2_events
        );
    }
}
