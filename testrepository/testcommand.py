#
# Copyright (c) 2010 Testrepository Contributors
# 
# Licensed under either the Apache License, Version 2.0 or the BSD 3-clause
# license at the users choice. A copy of both licenses are available in the
# project source as Apache-2.0 and BSD. You may not use this file except in
# compliance with one of these two licences.
# 
# Unless required by applicable law or agreed to in writing, software
# distributed under these licenses is distributed on an "AS IS" BASIS, WITHOUT
# WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.  See the
# license you chose for the specific language governing permissions and
# limitations under that license.

"""The test command that test repository knows how to run."""

from extras import (
    try_import,
    try_imports,
    )

from collections import defaultdict
ConfigParser = try_imports(['ConfigParser', 'configparser'])
from functools import partial
import io
import itertools
import operator
import os.path
import re
import subprocess
import sys
import tempfile
import multiprocessing
from textwrap import dedent

from fixtures import Fixture
v2 = try_import('subunit.v2')

from testrepository import results
from testrepository._computecontext import (
    Cache,
    Instance,
    )
from testrepository.testlist import (
    parse_enumeration,
    write_list,
    )

testrconf_help = dedent("""
    Configuring via .testr.conf:
    ---
    [DEFAULT]
    test_command=foo $IDOPTION
    test_id_option=--bar $IDFILE
    ---
    will cause 'testr run' to run 'foo' to execute tests, and
    'testr run --failing' will cause 'foo --bar failing.list ' to be run to
    execute tests. Shell variables are expanded in these commands on platforms
    that have a shell.

    The full list of options and variables for .testr.conf:
    * filter_tags -- a list of tags which should be used to filter test counts.
      This is useful for stripping out non-test results from the subunit stream
      such as Zope test layers. These filtered items are still considered for
      test failures.
    * test_command -- command line to run to execute tests.
    * test_id_option -- the value to substitute into test_command when specific
      test ids should be run.
    * test_id_list_default -- the value to use for $IDLIST when no specific
      test ids are being run.
    * test_list_option -- the option to use to cause the test runner to report
      on the tests it would run, rather than running them. When supplied the
      test_command should output on stdout all the test ids that would have
      been run if every other option and argument was honoured, one per line.
      This is required for parallel testing, and is substituted into $LISTOPT.
    * test_run_concurrency -- Optional call out to establish concurrency.
      Should return one line containing the number of concurrent test runner
      processes to run.
    * instance_provision -- provision one or more test run environments.
      Accepts $INSTANCE_COUNT for the number of instances desired.
    * instance_execute -- execute a test runner process in a given environment.
      Accepts $INSTANCE_ID, $FILES and $COMMAND. Paths in $FILES should be
      synchronised into the test runner environment filesystem. $COMMAND can
      be adjusted if the paths are synched with different names.
    * instance_dispose -- dispose of one or more test running environments.
      Accepts $INSTANCE_IDS.
    * group_regex -- If set group tests by the matched section of the test id.
    * $IDOPTION -- the variable to use to trigger running some specific tests.
    * $IDFILE -- A file created before the test command is run and deleted
      afterwards which contains a list of test ids, one per line. This can
      handle test ids with emedded whitespace.
    * $IDLIST -- A list of the test ids to run, separated by spaces. IDLIST
      defaults to an empty string when no test ids are known and no explicit
      default is provided. This will not handle test ids with spaces.

    See the testrepository manual for example .testr.conf files in different
    programming languages.

    """)


class CallWhenProcFinishes(object):
    """Convert a process object to trigger a callback when returncode is set.
    
    This just wraps the entire object and when the returncode attribute access
    finds a set value, calls the callback.
    """

    def __init__(self, process, callback):
        """Adapt process

        :param process: A subprocess.Popen object.
        :param callback: The process to call when the process completes.
        """
        self._proc = process
        self._callback = callback
        self._done = False

    @property
    def stdin(self):
        return self._proc.stdin

    @property
    def stdout(self):
        return self._proc.stdout

    @property
    def stderr(self):
        return self._proc.stderr

    @property
    def returncode(self):
        result = self._proc.returncode
        if not self._done and result is not None:
            self._done = True
            self._callback()
        return result

    def wait(self):
        return self._proc.wait()


def _callout(key, parser, ui):
    """Make a callout."""
    try:
        cmd = parser.get('DEFAULT', key)
    except ConfigParser.NoOptionError:
        return None
    run_proc = ui.subprocess_Popen(cmd, shell=True,
        stdout=subprocess.PIPE, stdin=subprocess.PIPE)
    out, err = run_proc.communicate()
    if run_proc.returncode:
        raise ValueError(
            "%s failed: exit code %d, stderr='%s'" % (
            key, run_proc.returncode, err.decode('utf8', 'replace')))
    return out


def _callout_concurrency(parser, ui):
    """Callout for user defined concurrency."""
    out = _callout('test_run_concurrency', parser, ui)
    if out is None:
        return None
    return int(out.strip())


def _default_profiles(parser, ui):
    """Callout to determine profiles to use by default."""
    out = _callout('default_profiles', parser, ui)
    if out is None:
        return set()
    return set([s for s in out.split() if s])


def _list_profiles(parser, ui):
    """Callout to determine available profiles."""
    out = _callout('list_profiles', parser, ui)
    if out is None:
        return set()
    return set([s for s in out.split() if s])


def local_concurrency():
    try:
        return multiprocessing.cpu_count()
    except NotImplementedError:
        # No concurrency logic known.
        return None


compiled_re_type = type(re.compile(''))


class RunInInstance(Fixture):
    """Knows how to run commands in an instance.
    
    This involves:
     - variable substitutions for INSTANCE_ID and INSTANCE_PROFILE only.
     - allocating and releasing the instance around a command
     - copying files into the remote instance
    
    This does not involve:
     - managing the instance lifecycle
    """

    def __init__(self, ui, instance_source, config):
        """Create a RunInInstance.

        :param ui: The ui object to provide user feedback and perform Popen.
        :param instance_source: An instance source.
        :param config: The ConfigParser configuration.
        """
        super(RunInInstance, self).__init__()
        self._ui = ui
        self._instance_source = instance_source
        self._parser = config

    def _setUp(self):
        self._instance = self._instance_source.obtain_instance()
        if self._instance is not None:
            self.addCleanup(
                self._instance_source.release_instance, self._instance)

    def _create_cmd(self, cmd, copyfile):
        """Peform variable substition for cmd.
        
        :param cmd: The string command to run.
        :param copyfile: None or a file to copy.
        """
        if self._instance is None:
            return cmd
        try:
            instance_prefix = self._parser.get('DEFAULT', 'instance_execute')
            variables = {
                'INSTANCE_ID': self._instance.id,
                'COMMAND': cmd,
                # --list-tests cannot use FILES, so handle it being unset.
                'FILES': copyfile or '',
            }
            variable_regex = '\$(INSTANCE_ID|COMMAND|FILES)'
            def subst(match):
                return variables.get(match.groups(1)[0], '')
            cmd = re.sub(variable_regex, subst, instance_prefix)
        except ConfigParser.NoOptionError:
            # Per-instance execution environment not configured.
            pass
        return cmd

    def spawn(self, cmd, copyfile):
        """Spawn cmd in the environment.

        :param cmd: The string command to spawn.
        :param copyfile: None or the name of a local file to copy into the
            environment.
        """
        cmd = self._create_cmd(cmd, copyfile)
        self._ui.output_values([('running', cmd)])
        run_proc = self._ui.subprocess_Popen(cmd, shell=True,
            stdout=subprocess.PIPE, stdin=subprocess.PIPE)
        return run_proc


class TestListingFixture(Fixture):
    """Write a temporary file to disk with test ids in it."""

    def __init__(self, test_ids, cmd_template, listopt, idoption, ui,
        repository, parallel=True, listpath=None, parser=None,
        test_filters=None, instance_source=None, group_callback=None,
        concurrency=None):
        """Create a TestListingFixture.

        :param test_ids: The test_ids to use. May be None indicating that
            no ids are known and they should be discovered by listing or
            configuration if they must be known to run tests. Test ids are
            needed to run tests when filtering or partitioning is needed: if
            the run concurrency is > 1 partitioning is needed, and filtering is
            needed if the user has passed in filters.
        :param cmd_template: string to be filled out with
            IDFILE.
        :param listopt: Option to substitute into LISTOPT to cause test listing
            to take place.
        :param idoption: Option to substitutde into cmd when supplying any test
            ids.
        :param ui: The UI in use.
        :param repository: The repository to query for test times, if needed.
        :param parallel: If not True, prohibit parallel use : used to implement
            --parallel run recursively.
        :param listpath: The file listing path to use. If None, a unique path
            is created.
        :param parser: An options parser for reading options from.
        :param test_filters: An optional list of test filters to apply. Each
            filter should be a string suitable for passing to re.compile.
            filters are applied using search() rather than match(), so if
            anchoring is needed it should be included in the regex.
            The test ids used for executing are the union of all the individual
            filters: to take the intersection instead, craft a single regex that
            matches all your criteria. Filters are automatically applied by
            run_tests(), or can be applied by calling filter_tests(test_ids).
        :param instance_source: A source of test run instances. Must support
            obtain_instance() -> id and release_instance(id)
            calls.
        :param group_callback: If supplied, should be a function that accepts a
            test id and returns a group id. A group id is an arbitrary value
            used as a dictionary key in the scheduler. All test ids with the
            same group id are scheduled onto the same backend test process.
        """
        self.test_ids = test_ids
        self.template = cmd_template
        self.listopt = listopt
        self.idoption = idoption
        self.ui = ui
        self.repository = repository
        self.parallel = parallel
        self._listpath = listpath
        self._parser = parser
        self.test_filters = test_filters
        self._group_callback = group_callback
        assert instance_source is not None
        self._instance_source = instance_source
        assert concurrency is not None
        self.concurrency = concurrency

    def setUp(self):
        super(TestListingFixture, self).setUp()
        variable_regex = '\$(IDOPTION|IDFILE|IDLIST|LISTOPT)'
        variables = {}
        list_variables = {'LISTOPT': self.listopt}
        cmd = self.template
        try:
            default_idstr = self._parser.get('DEFAULT', 'test_id_list_default')
            list_variables['IDLIST'] = default_idstr
            # In theory we should also support casting this into IDFILE etc -
            # needs this horrible class refactored.
        except ConfigParser.NoOptionError as e:
            if e.message != "No option 'test_id_list_default' in section: 'DEFAULT'":
                raise
            default_idstr = None
        def list_subst(match):
            return list_variables.get(match.groups(1)[0], '')
        self.list_cmd = re.sub(variable_regex, list_subst, cmd)
        if self.test_ids is None:
            if self.concurrency == 1:
                if default_idstr:
                    self.test_ids = default_idstr.split()
            if self.concurrency != 1 or self.test_filters is not None:
                # Have to be able to tell each worker what to run / filter
                # tests.
                self.test_ids = self.list_tests()
        if self.test_ids is None:
            # No test ids to supply to the program.
            self.list_file_name = None
            name = ''
            idlist = ''
        else:
            self.test_ids = self.filter_tests(self.test_ids)
            name = self._make_listfile()
            variables['IDFILE'] = name
            idlist = ' '.join(self.test_ids)
        variables['IDLIST'] = idlist
        def subst(match):
            return variables.get(match.groups(1)[0], '')
        if self.test_ids is None:
            # No test ids, no id option.
            idoption = ''
        else:
            idoption = re.sub(variable_regex, subst, self.idoption)
            variables['IDOPTION'] = idoption
        self.cmd = re.sub(variable_regex, subst, cmd)

    def _make_listfile(self):
        name = None
        try:
            if self._listpath:
                name = self._listpath
                stream = open(name, 'wb')
            else:
                fd, name = tempfile.mkstemp()
                stream = os.fdopen(fd, 'wb')
            self.list_file_name = name
            write_list(stream, self.test_ids)
            stream.close()
        except:
            if name:
                os.unlink(name)
            raise
        self.addCleanup(os.unlink, name)
        return name

    def filter_tests(self, test_ids):
        """Filter test_ids by the test_filters.
        
        :return: A list of test ids.
        """
        if self.test_filters is None:
            return test_ids
        filters = list(map(re.compile, self.test_filters))
        def include(test_id):
            for pred in filters:
                if pred.search(test_id):
                    return True
        return list(filter(include, test_ids))

    # XXX Entry point for list_tests command.
    #     Use cases: - all tests across all profiles [testr list-tests], testr run --isolated
    #                - tests for one profile for scheduling?
    def list_tests(self):
        """List the tests returned by list_cmd.

        :return: A list of test ids.
        """
        if '$LISTOPT' not in self.template:
            raise ValueError("LISTOPT not configured in .testr.conf")
        with RunInInstance(self.ui, self._instance_source, self._parser) as runner:
            run_proc = runner.spawn(
                self.list_cmd, getattr(self, 'list_file_name', None))
            out, err = run_proc.communicate()
            if run_proc.returncode != 0:
                if v2 is not None:
                    new_out = io.BytesIO()
                    v2.ByteStreamToStreamResult(io.BytesIO(out), 'stdout').run(
                        results.CatFiles(new_out))
                    out = new_out.getvalue()
                self.ui.output_stream(io.BytesIO(out))
                self.ui.output_stream(io.BytesIO(err))
                raise ValueError(
                    "Non-zero exit code (%d) from test listing."
                    % (run_proc.returncode))
            ids = parse_enumeration(out)
            return ids

    def _per_instance_command(self, cmd, runner):
        """Customise cmd to with an instance-id.
        
        :param concurrency: The number of instances to ask for (used to avoid
            death-by-1000 cuts of latency.
        """
        return runner._create_cmd(cmd, getattr(self, 'list_file_name', None))

    # entry point, for self, and for commands.run.
    def run_tests(self):
        """Run the tests defined by the command and ui.

        :return: A list of spawned processes.
        """
        result = []
        test_ids = self.test_ids
        if self.concurrency == 1 and (test_ids is None or test_ids):
            # Have to customise cmd here, as instances are allocated
            # just-in-time. XXX: Indicates this whole region needs refactoring.
            runner = RunInInstance(self.ui, self._instance_source, self._parser)
            try:
                runner.setUp()
                run_proc = runner.spawn(self.cmd, getattr(self, 'list_file_name', None))
                # Prevent processes stalling if they read from stdin; we could
                # pass this through in future, but there is no point doing that
                # until we have a working can-run-debugger-inline story.
                run_proc.stdin.close()
                return [CallWhenProcFinishes(run_proc, runner.cleanUp)]
            except:
                runner.cleanUp()
                raise
        test_id_groups = self.partition_tests(test_ids, self.concurrency)
        for test_ids in test_id_groups:
            if not test_ids:
                # No tests in this partition
                continue
            fixture = self.useFixture(TestListingFixture(test_ids,
                self.template, self.listopt, self.idoption, self.ui,
                self.repository, parallel=False, parser=self._parser,
                instance_source=self._instance_source,
                concurrency=1))
            result.extend(fixture.run_tests())
        return result

    def partition_tests(self, test_ids, concurrency):
        """Parition test_ids by concurrency.

        Test durations from the repository are used to get partitions which
        have roughly the same expected runtime. New tests - those with no
        recorded duration - are allocated in round-robin fashion to the 
        partitions created using test durations.

        :return: A list where each element is a distinct subset of test_ids,
            and the union of all the elements is equal to set(test_ids).
        """
        partitions = [list() for i in range(concurrency)]
        timed_partitions = [[0.0, partition] for partition in partitions]
        time_data = self.repository.get_test_times(test_ids)
        timed_tests = time_data['known']
        unknown_tests = time_data['unknown']
        # Group tests: generate group_id -> test_ids.
        group_ids = defaultdict(list)
        if self._group_callback is None:
            group_callback = lambda _:None
        else:
            group_callback = self._group_callback
        for test_id in test_ids:
            group_id = group_callback(test_id) or test_id
            group_ids[group_id].append(test_id)
        # Time groups: generate three sets of groups:
        # - fully timed dict(group_id -> time),
        # - partially timed dict(group_id -> time) and
        # - unknown (set of group_id)
        # We may in future treat partially timed different for scheduling, but
        # at least today we just schedule them after the fully timed groups.
        timed = {}
        partial = {}
        unknown = []
        for group_id, group_tests in group_ids.items():
            untimed_ids = unknown_tests.intersection(group_tests)
            group_time = sum([timed_tests[test_id]
                for test_id in untimed_ids.symmetric_difference(group_tests)])
            if not untimed_ids:
                timed[group_id] = group_time
            elif group_time:
                partial[group_id] = group_time
            else:
                unknown.append(group_id)
        # Scheduling is NP complete in general, so we avoid aiming for
        # perfection. A quick approximation that is sufficient for our general
        # needs:
        # sort the groups by time
        # allocate to partitions by putting each group in to the partition with
        # the current (lowest time, shortest length[in tests])
        def consume_queue(groups):
            queue = sorted(
                groups.items(), key=operator.itemgetter(1), reverse=True)
            for group_id, duration in queue:
                timed_partitions[0][0] = timed_partitions[0][0] + duration
                timed_partitions[0][1].extend(group_ids[group_id])
                timed_partitions.sort(key=lambda item:(item[0], len(item[1])))
        consume_queue(timed)
        consume_queue(partial)
        # Assign groups with entirely unknown times in round robin fashion to
        # the partitions. 
        for partition, group_id in zip(itertools.cycle(partitions), unknown):
            partition.extend(group_ids[group_id])
        return partitions


class TestCommand(Fixture):
    """Represents the test command defined in .testr.conf.
    
    :ivar run_factory: The fixture to use to execute a command.
    :ivar oldschool: Use failing.list rather than a unique file path.

    TestCommand is a Fixture. Many uses of it will not require it to be setUp,
    but calling get_run_command does require it: the fixture state is used to
    track test environment instances, which are disposed of when cleanUp
    happens. This is not done per-run-command, because test bisection (amongst
    other things) uses multiple get_run_command configurations.
    """
    
    run_factory = TestListingFixture
    oldschool = False

    def __init__(self, ui, repository):
        """Create a TestCommand.

        :param ui: A testrepository.ui.UI object which is used to obtain the
            location of the .testr.conf.
        :param repository: A testrepository.repository.Repository used for
            determining test times when partitioning tests.
        """
        super(TestCommand, self).__init__()
        self.ui = ui
        self.repository = repository
        self._instance_cache = None

    def setUp(self):
        super(TestCommand, self).setUp()
        self._instance_cache = Cache()
        self.addCleanup(self._dispose_instances)
        nonparallel = (
            not getattr(self.ui, 'options', None) or
            not getattr(self.ui.options, 'parallel', None))
        if nonparallel:
            self.concurrency = 1
        else:
            self.concurrency = self.ui.options.concurrency
            if not self.concurrency:
                self.concurrency = _callout_concurrency(self.get_parser(), self.ui)
            if not self.concurrency:
                self.concurrency = local_concurrency()
            if not self.concurrency:
                self.concurrency = 1

    def _dispose_instances(self):
        cache = self._instance_cache
        if cache is None:
            return
        self._instance_cache = None
        try:
            dispose_cmd = self.get_parser().get('DEFAULT', 'instance_dispose')
        except (ValueError, ConfigParser.NoOptionError):
            # User has said we don't need to dispose of instances.
            return
        variable_regex = '\$INSTANCE_IDS'
        instances = cache.all()
        dispose_cmd = re.sub(
            variable_regex,
            ' '.join(sorted(instance.id for instance in instances)),
            dispose_cmd)
        self.ui.output_values([('running', dispose_cmd)])
        run_proc = self.ui.subprocess_Popen(dispose_cmd, shell=True)
        run_proc.communicate()
        if run_proc.returncode:
            raise ValueError('Disposing of instances failed, return %d' %
                run_proc.returncode)

    def get_parser(self):
        """Get a parser with the .testr.conf in it."""
        parser = ConfigParser.ConfigParser()
        # This possibly should push down into UI.
        if self.ui.here == 'memory:':
            return parser
        if not parser.read(os.path.join(self.ui.here, '.testr.conf')):
            raise ValueError("No .testr.conf config file")
        return parser

    def get_run_command(self, test_ids=None, testargs=(), test_filters=None):
        """Get the command that would be run to run tests.
        
        See TestListingFixture for the definition of test_ids and test_filters.
        """
        if self._instance_cache is None:
            raise TypeError('TestCommand not setUp')
        parser = self.get_parser()
        try:
            command = parser.get('DEFAULT', 'test_command')
        except ConfigParser.NoOptionError as e:
            if e.message != "No option 'test_command' in section: 'DEFAULT'":
                raise
            raise ValueError("No test_command option present in .testr.conf")
        elements = [command] + list(testargs)
        cmd = ' '.join(elements)
        idoption = ''
        if '$IDOPTION' in command:
            # IDOPTION is used, we must have it configured.
            try:
                idoption = parser.get('DEFAULT', 'test_id_option')
            except ConfigParser.NoOptionError as e:
                if e.message != "No option 'test_id_option' in section: 'DEFAULT'":
                    raise
                raise ValueError("No test_id_option option present in .testr.conf")
        listopt = ''
        if '$LISTOPT' in command:
            # LISTOPT is used, test_list_option must be configured.
            try:
                listopt = parser.get('DEFAULT', 'test_list_option')
            except ConfigParser.NoOptionError as e:
                if e.message != "No option 'test_list_option' in section: 'DEFAULT'":
                    raise
                raise ValueError("No test_list_option option present in .testr.conf")
        try:
            group_regex = parser.get('DEFAULT', 'group_regex')
        except ConfigParser.NoOptionError:
            group_regex = None
        if group_regex:
            def group_callback(test_id, regex=re.compile(group_regex)):
                match = regex.match(test_id)
                if match:
                    return match.group(0)
        else:
            group_callback = None
        if self.oldschool:
            listpath = os.path.join(self.ui.here, 'failing.list')
            run_factory = partial(self.run_factory, listpath=listpath)
        else:
            run_factory = self.run_factory
        result = run_factory(test_ids, cmd, listopt, idoption,
            self.ui, self.repository, parser=parser,
            test_filters=test_filters, instance_source=self,
            group_callback=group_callback, concurrency=self.concurrency)
        return result

    def get_filter_tags(self):
        parser = self.get_parser()
        try:
            tags = parser.get('DEFAULT', 'filter_tags')
        except ConfigParser.NoOptionError as e:
            if e.message != "No option 'filter_tags' in section: 'DEFAULT'":
                raise
            return set()
        return set([tag.strip() for tag in tags.split()])

    def obtain_instance(self):
        """If possible, get one or more test run environment instance ids.
        
        Note this is not threadsafe: calling it from multiple threads would
        likely result in shared results.
        """
        profile = u"DEFAULT"
        while self._instance_cache.size() < self.concurrency:
            try:
                cmd = self.get_parser().get('DEFAULT', 'instance_provision')
            except ConfigParser.NoOptionError:
                # Instance allocation not configured
                return None
            variable_regex = '\$INSTANCE_COUNT'
            cmd = re.sub(variable_regex,
                str(self.concurrency - self._instance_cache.size()), cmd)
            self.ui.output_values([('running', cmd)])
            proc = self.ui.subprocess_Popen(
                cmd, shell=True, stdout=subprocess.PIPE)
            out, _ = proc.communicate()
            if proc.returncode:
                raise ValueError('Provisioning instances failed, return %d' %
                    proc.returncode)
            out = out.decode('utf-8')
            new_instances = set([Instance(profile, item.strip()) for item in out.split()])
            list(map(self._instance_cache.add, new_instances))
        # We only ask for instances when one should be available.
        return self._instance_cache.allocate(profile)

    def release_instance(self, instance):
        """Return instance_ids to the pool for reuse."""
        self._instance_cache.release(instance)
