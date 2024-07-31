use tempfile::{tempdir, TempDir};
use testrepository_repository::{
    file::File,
    implementations::{OpenOptions, Repository},
    memory::MemoryStore,
};
use url::Url;

mod count;
mod latest_id;

enum Implementation {
    Memory,
    RepoV1,
    RepoV2,
}

impl Implementation {
    async fn setup(&self) -> TestGuard {
        match self {
            Implementation::Memory => {
                let mut store = MemoryStore::default();
                // Only allow one repo for now
                store.initialize("a");
                TestGuard::Memory(store)
            }
            Implementation::RepoV1 => {
                let dir = tempdir().unwrap();
                #[allow(deprecated)]
                File::initialize_v1(dir.path()).await.unwrap();
                TestGuard::RepoV1(dir)
            }
            Implementation::RepoV2 => {
                let dir = tempdir().unwrap();
                File::initialize_v2(dir.path()).await.unwrap();
                TestGuard::RepoV2(dir)
            }
        }
    }
}

enum TestGuard {
    Memory(MemoryStore),
    RepoV1(TempDir),
    RepoV2(TempDir),
}

impl TestGuard {
    async fn open(&self) -> Repository {
        match self {
            TestGuard::Memory(store) => {
                let opts = OpenOptions::default().with_memory_store(store);
                Repository::open_with(&Url::parse("memory://a").unwrap(), opts)
                    .await
                    .unwrap()
            }
            TestGuard::RepoV1(dir) => {
                let url = Url::from_file_path(dir.path()).unwrap();
                Repository::open(&url).await.unwrap()
            }
            TestGuard::RepoV2(dir) => {
                let url = Url::from_file_path(dir.path()).unwrap();
                Repository::open(&url).await.unwrap()
            }
        }
    }
}
