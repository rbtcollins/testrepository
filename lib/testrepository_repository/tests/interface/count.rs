use test_case::test_matrix;
use tracing_test::traced_test;

use testrepository_repository::repository::Repository as _;

use super::Implementation;

#[test_matrix(
        [Implementation::Memory, Implementation::RepoV1, Implementation::RepoV2]
    )]
#[tokio::test]
#[traced_test]
async fn count_empty_repo(implementation: Implementation) {
    let guard = implementation.setup().await;
    let repo = guard.open().await;
    assert_eq!(repo.count().await.unwrap(), 0);
}
