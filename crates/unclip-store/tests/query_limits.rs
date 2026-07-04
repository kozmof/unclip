use sea_orm::ConnectionTrait;
use unclip_core::SampleQuery;
use unclip_store::{BranchRepository, BranchRepositoryError, SeaOrmBranchRepository};

#[tokio::test]
async fn broad_find_fails_before_hydrating_an_excessive_archive() {
    let db = unclip_store::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    db.execute_unprepared(
        "WITH RECURSIVE seq(n) AS (\
           VALUES(1) UNION ALL SELECT n + 1 FROM seq WHERE n < 10001\
         )\
         INSERT INTO branches \
           (path, weight, created_at, updated_at)\
         SELECT printf('/branch-%05d', n), 1.0, \
                '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'\
         FROM seq",
    )
    .await
    .unwrap();

    let repo = SeaOrmBranchRepository::new(db);
    let error = repo.find(SampleQuery::default()).await.unwrap_err();
    assert!(
        matches!(
            error,
            BranchRepositoryError::QueryTooBroad { limit: 10_000 }
        ),
        "unexpected error: {error}"
    );

    // Bulk consumers can deliberately traverse the same result in bounded,
    // stable pages rather than being blocked by the sampling safety limit.
    let all = repo.find_all(SampleQuery::default()).await.unwrap();
    assert_eq!(all.len(), 10_001);
    assert_eq!(all.first().unwrap().path, "/branch-00001");
    assert_eq!(all.last().unwrap().path, "/branch-10001");
}
