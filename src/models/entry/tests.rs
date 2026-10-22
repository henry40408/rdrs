use super::*;
use crate::models::category;
use crate::models::feed;
use crate::test_support::{create_test_user, setup_db};
use chrono::TimeZone;

fn ewf_with_base_parts(link: Option<&str>, site_url: Option<&str>) -> EntryWithFeed {
    let now = Utc::now();
    EntryWithFeed {
        entry: Entry {
            id: 1,
            feed_id: 1,
            guid: "g".to_string(),
            title: None,
            link: link.map(str::to_string),
            content: None,
            full_content: None,
            summary: None,
            author: None,
            published_at: Some(now),
            read_at: None,
            starred_at: None,
            created_at: now,
            updated_at: now,
        },
        feed_title: None,
        feed_url: "https://feed.example/atom.xml".to_string(),
        site_url: site_url.map(str::to_string),
        category_id: 1,
        category_name: "Cat".to_string(),
        feed_has_icon: false,
        custom_referrer: None,
    }
}

#[test]
fn content_base_url_prefers_the_entry_link() {
    let entry_with_feed = ewf_with_base_parts(
        Some("https://site.example/post"),
        Some("https://site.example/"),
    );
    assert_eq!(
        entry_with_feed.content_base_url(),
        "https://site.example/post"
    );
}

#[test]
fn content_base_url_falls_back_to_site_then_feed() {
    let entry_with_feed = ewf_with_base_parts(None, Some("https://site.example/"));
    assert_eq!(entry_with_feed.content_base_url(), "https://site.example/");

    let entry_with_feed = ewf_with_base_parts(None, None);
    assert_eq!(
        entry_with_feed.content_base_url(),
        "https://feed.example/atom.xml"
    );
}

#[test]
fn content_base_url_treats_empty_strings_as_absent() {
    // Nullable columns can also hold "".
    let entry_with_feed = ewf_with_base_parts(Some(""), Some(""));
    assert_eq!(
        entry_with_feed.content_base_url(),
        "https://feed.example/atom.xml"
    );
}

async fn create_test_category(db: &Db, user_id: i64, name: &str) -> i64 {
    category::create_category(db, user_id, name)
        .await
        .unwrap()
        .id
}

async fn create_test_feed(db: &Db, category_id: i64, url: &str) -> i64 {
    feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id,
            url,
            title: Some("Test Feed"),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
}

/// `count` entries, oldest first (last id is newest).
async fn seed_offline_entries(db: &Db, feed_id: i64, count: usize) -> Vec<i64> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let (entry, _) = upsert_entry(
            db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                content: Some("body"),
                published_at: Some(Utc::now() - chrono::Duration::minutes((count - i) as i64)),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        ids.push(entry.id);
    }
    ids
}

#[tokio::test]
async fn offline_set_is_empty_when_the_budget_is_zero() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "reader").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    seed_offline_entries(&db, feed_id, 3).await;

    assert!(list_offline_set(&db, user_id, 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn offline_set_takes_the_newest_unread_first() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "reader").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let ids = seed_offline_entries(&db, feed_id, 5).await;

    let set = list_offline_set(&db, user_id, 2).await.unwrap();
    let got: Vec<i64> = set.iter().map(|e| e.entry.id).collect();
    assert_eq!(got, vec![ids[4], ids[3]]);
}

#[tokio::test]
async fn offline_set_fills_the_remaining_budget_with_starred() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "reader").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let ids = seed_offline_entries(&db, feed_id, 4).await;

    // Oldest: read but starred, so it fills the leftover budget.
    mark_as_read(&db, ids[0]).await.unwrap();
    star_entry(&db, ids[0]).await.unwrap();

    let set = list_offline_set(&db, user_id, 4).await.unwrap();
    let got: Vec<i64> = set.iter().map(|e| e.entry.id).collect();
    assert!(
        got.contains(&ids[0]),
        "a starred read entry belongs offline"
    );
    assert_eq!(got.len(), 4, "three unread plus the starred one");
    // Still newest-first overall, so the list reads like every other one.
    assert_eq!(got[0], ids[3]);
}

#[tokio::test]
async fn offline_set_never_repeats_an_entry_that_is_both() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "reader").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let ids = seed_offline_entries(&db, feed_id, 2).await;

    // Unread *and* starred: must not appear twice.
    star_entry(&db, ids[0]).await.unwrap();

    let set = list_offline_set(&db, user_id, 10).await.unwrap();
    let got: Vec<i64> = set.iter().map(|e| e.entry.id).collect();
    assert_eq!(got.len(), 2);
    assert_eq!(got.iter().filter(|id| **id == ids[0]).count(), 1);
}

#[tokio::test]
async fn offline_set_is_scoped_to_the_reader() {
    let db = setup_db().await;
    let mine = create_test_user(&db, "reader").await;
    let theirs = create_test_user(&db, "other").await;
    let my_cat = create_test_category(&db, mine, "Tech").await;
    let their_cat = create_test_category(&db, theirs, "Tech").await;
    let my_feed = create_test_feed(&db, my_cat, "https://example.com/a.xml").await;
    let their_feed = create_test_feed(&db, their_cat, "https://example.com/b.xml").await;
    let my_ids = seed_offline_entries(&db, my_feed, 2).await;
    let their_ids = seed_offline_entries(&db, their_feed, 2).await;

    let got: Vec<i64> = list_offline_set(&db, mine, 50)
        .await
        .unwrap()
        .iter()
        .map(|e| e.entry.id)
        .collect();

    assert_eq!(got.len(), my_ids.len());
    for id in their_ids {
        assert!(!got.contains(&id));
    }
}

#[tokio::test]
async fn test_upsert_entry() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let (entry, is_new) = upsert_entry(
        &db,
        &EntryUpsert {
            feed_id,
            guid: "guid-123",
            title: Some("Test Entry"),
            link: Some("https://example.com/entry"),
            content: Some("Content"),
            summary: Some("Summary"),
            author: Some("Author"),
            published_at: Some(Utc::now()),
        },
    )
    .await
    .unwrap();

    assert!(is_new);
    assert_eq!(entry.title, Some("Test Entry".to_string()));
    assert!(entry.read_at.is_none());
    assert!(entry.starred_at.is_none());

    let (updated, is_new) = upsert_entry(
        &db,
        &EntryUpsert {
            feed_id,
            guid: "guid-123",
            title: Some("Updated Title"),
            link: Some("https://example.com/entry"),
            content: Some("Updated Content"),
            summary: None,
            author: None,
            published_at: None,
        },
    )
    .await
    .unwrap();

    assert!(!is_new);
    assert_eq!(updated.title, Some("Updated Title".to_string()));
    assert_eq!(updated.id, entry.id);
}

#[tokio::test]
async fn test_mark_as_read() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let (entry, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Test"),
            ..EntryUpsert::new(feed_id, "guid-123")
        },
    )
    .await
    .unwrap();

    assert!(entry.read_at.is_none());

    let read = mark_as_read(&db, entry.id).await.unwrap();
    assert!(read.read_at.is_some());
}

#[tokio::test]
async fn unread_counts_keep_zero_feeds_and_empty_categories() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let busy_cat = create_test_category(&db, user_id, "Busy").await;
    let empty_cat = create_test_category(&db, user_id, "Empty").await;
    let busy_feed = create_test_feed(&db, busy_cat, "https://example.com/busy.xml").await;
    let quiet_feed = create_test_feed(&db, busy_cat, "https://example.com/quiet.xml").await;
    let mut ids = Vec::new();
    for guid in ["a", "b", "c"] {
        let (e, _) = upsert_entry(&db, &EntryUpsert::new(busy_feed, guid))
            .await
            .unwrap();
        ids.push(e.id);
    }
    mark_as_read(&db, ids[0]).await.unwrap();

    let by_feed = count_unread_by_feed(&db, user_id).await.unwrap();
    assert_eq!(by_feed.get(&busy_feed), Some(&2));
    assert_eq!(by_feed.get(&quiet_feed), Some(&0));

    let in_category = count_unread_by_feed_in_category(&db, user_id, busy_cat)
        .await
        .unwrap();
    assert_eq!(in_category.len(), 2);
    assert_eq!(in_category.get(&busy_feed), Some(&2));

    let by_category = count_unread_by_category(&db, user_id).await.unwrap();
    assert_eq!(by_category.get(&busy_cat), Some(&2));
    assert_eq!(by_category.get(&empty_cat), Some(&0));

    let other = create_test_user(&db, "other").await;
    assert!(
        count_unread_by_feed_in_category(&db, other, busy_cat)
            .await
            .unwrap()
            .is_empty(),
        "another account must not read this category's counts"
    );
}

#[tokio::test]
async fn is_owned_by_is_true_only_for_the_owner() {
    let db = setup_db().await;
    let owner = create_test_user(&db, "owner").await;
    let other = create_test_user(&db, "other").await;
    let category_id = create_test_category(&db, owner, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let (entry, _) = upsert_entry(&db, &EntryUpsert::new(feed_id, "guid-owned"))
        .await
        .unwrap();

    assert!(is_owned_by(&db, owner, entry.id).await.unwrap());
    assert!(!is_owned_by(&db, other, entry.id).await.unwrap());
    assert!(!is_owned_by(&db, owner, entry.id + 1000).await.unwrap());
}

#[tokio::test]
async fn mark_as_read_quietly_sets_read_at_once() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let (entry, _) = upsert_entry(&db, &EntryUpsert::new(feed_id, "guid-quiet"))
        .await
        .unwrap();

    mark_as_read_quietly(&db, entry.id).await.unwrap();
    let first = find_by_id(&db, entry.id).await.unwrap().unwrap().read_at;
    assert!(first.is_some());

    mark_as_read_quietly(&db, entry.id).await.unwrap();
    let second = find_by_id(&db, entry.id).await.unwrap().unwrap().read_at;
    assert_eq!(first, second);
}

#[tokio::test]
async fn mark_read_by_filter_marks_only_matching_unread_owned() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let m = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("超少女登場"),
            ..EntryUpsert::new(feed_id, "m")
        },
    )
    .await
    .unwrap();
    let n = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("其他新聞"),
            ..EntryUpsert::new(feed_id, "n")
        },
    )
    .await
    .unwrap();
    let UpsertOutcome::Inserted(m_id) = m else {
        panic!()
    };
    let UpsertOutcome::Inserted(n_id) = n else {
        panic!()
    };

    let filter = EntryFilter {
        feed_id: Some(feed_id),
        search: Some("超少女".to_string()),
        ..Default::default()
    };
    let affected = mark_read_by_filter(&db, user_id, &filter).await.unwrap();
    assert_eq!(affected, 1);

    let m_read = crate::query_scalar!(
        &db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        m_id
    )
    .unwrap();
    let n_read = crate::query_scalar!(
        &db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        n_id
    )
    .unwrap();
    assert!(m_read.is_some(), "matching entry marked read");
    assert!(n_read.is_none(), "non-matching entry untouched");

    // Idempotent: already-read matching row isn't recounted.
    assert_eq!(mark_read_by_filter(&db, user_id, &filter).await.unwrap(), 0);
}

#[tokio::test]
async fn mark_read_by_filter_does_not_cross_user_boundary() {
    let db = setup_db().await;
    let user1 = create_test_user(&db, "user1").await;
    let user2 = create_test_user(&db, "user2").await;
    let category1 = create_test_category(&db, user1, "Tech1").await;
    let category2 = create_test_category(&db, user2, "Tech2").await;
    let feed1 = create_test_feed(&db, category1, "https://example.com/feed1.xml").await;
    let feed2 = create_test_feed(&db, category2, "https://example.com/feed2.xml").await;

    // No feed/category scoping: ownership comes only from the user_id seed condition.
    let e1 = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("超少女登場"),
            ..EntryUpsert::new(feed1, "e1")
        },
    )
    .await
    .unwrap();
    let e2 = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("超少女登場"),
            ..EntryUpsert::new(feed2, "e2")
        },
    )
    .await
    .unwrap();
    let UpsertOutcome::Inserted(e1_id) = e1 else {
        panic!()
    };
    let UpsertOutcome::Inserted(e2_id) = e2 else {
        panic!()
    };

    let filter = EntryFilter {
        search: Some("超少女".to_string()),
        ..Default::default()
    };

    let affected = mark_read_by_filter(&db, user1, &filter).await.unwrap();
    assert_eq!(affected, 1);

    let e1_read = crate::query_scalar!(
        &db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        e1_id
    )
    .unwrap();
    let e2_read = crate::query_scalar!(
        &db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        e2_id
    )
    .unwrap();
    assert!(
        e1_read.is_some(),
        "owning user's matching entry marked read"
    );
    assert!(
        e2_read.is_none(),
        "matching entry belonging to a different user must not be marked read"
    );
}

#[tokio::test]
async fn test_count_unread() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let mut ids = Vec::new();
    for i in 0..5 {
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some("Test"),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        ids.push(entry.id);
    }

    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 5);

    mark_as_read(&db, ids[0]).await.unwrap();
    mark_as_read(&db, ids[1]).await.unwrap();

    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 3);
}

#[tokio::test]
async fn test_search_entries_by_title() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Rust Programming Guide"),
            content: Some("Content about Rust"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Python Tutorial"),
            content: Some("Content about Python"),
            ..EntryUpsert::new(feed_id, "guid-2")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        search: Some("Rust".to_string()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 10, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].entry.title,
        Some("Rust Programming Guide".to_string())
    );
}

#[tokio::test]
async fn test_search_entries_by_content() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Entry 1"),
            content: Some("This article discusses WebAssembly"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Entry 2"),
            content: Some("This article discusses JavaScript"),
            ..EntryUpsert::new(feed_id, "guid-2")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        search: Some("WebAssembly".to_string()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 10, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.title, Some("Entry 1".to_string()));
}

#[tokio::test]
async fn test_search_case_insensitive() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("UPPERCASE Title"),
            content: Some("lowercase content"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();

    // Search with lowercase should match uppercase title
    let filter = EntryFilter {
        search: Some("uppercase".to_string()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 10, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);

    // Search with uppercase should match lowercase content
    let filter = EntryFilter {
        search: Some("LOWERCASE".to_string()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 10, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_search_combined_with_filters() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let (entry1, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Rust Article"),
            content: Some("Content"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    let (entry2, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Rust Tutorial"),
            content: Some("Content"),
            ..EntryUpsert::new(feed_id, "guid-2")
        },
    )
    .await
    .unwrap();

    mark_as_read(&db, entry1.id).await.unwrap();
    star_entry(&db, entry2.id).await.unwrap();

    // Search for "Rust" with unread_only - should only return entry2
    let filter = EntryFilter {
        search: Some("Rust".to_string()),
        unread_only: true,
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 10, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.id, entry2.id);

    // Search for "Rust" with starred_only - should only return entry2
    let filter = EntryFilter {
        search: Some("Rust".to_string()),
        starred_only: true,
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 10, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.id, entry2.id);
}

#[tokio::test]
async fn test_search_pagination() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    for i in 0..5 {
        upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Rust Article {i}")),
                content: Some("Content"),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
    }

    let filter = EntryFilter {
        search: Some("Rust".to_string()),
        ..Default::default()
    };

    let results = list_by_user(&db, &page(user_id, &filter, 2, 0))
        .await
        .unwrap();
    assert_eq!(results.len(), 2);

    let results = list_by_user(&db, &page(user_id, &filter, 2, 2))
        .await
        .unwrap();
    assert_eq!(results.len(), 2);

    let results = list_by_user(&db, &page(user_id, &filter, 2, 4))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);

    let count = count_by_user(&db, user_id, &filter).await.unwrap();
    assert_eq!(count, 5);
}

#[tokio::test]
async fn query_is_unread_returns_only_unread() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let (alpha, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("alpha"),
            ..EntryUpsert::new(feed_id, "guid-alpha")
        },
    )
    .await
    .unwrap();
    let (beta, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("beta"),
            ..EntryUpsert::new(feed_id, "guid-beta")
        },
    )
    .await
    .unwrap();
    mark_as_read(&db, beta.id).await.unwrap();
    assert!(alpha.read_at.is_none());

    let filter = EntryFilter {
        query: Some(query::parse("is:unread").unwrap()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    let titles: Vec<_> = results
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(titles.iter().any(|t| t == "alpha"));
    assert!(!titles.iter().any(|t| t == "beta"));
}

#[tokio::test]
async fn query_boolean_and_negation() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("rust"),
            ..EntryUpsert::new(feed_id, "guid-rust")
        },
    )
    .await
    .unwrap();
    let (rust_weekly, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("rust weekly"),
            ..EntryUpsert::new(feed_id, "guid-rust-weekly")
        },
    )
    .await
    .unwrap();
    mark_as_read(&db, rust_weekly.id).await.unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("go"),
            ..EntryUpsert::new(feed_id, "guid-go")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        query: Some(query::parse("rust -is:read").unwrap()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    let titles: Vec<_> = results
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(titles.iter().any(|t| t == "rust"));
    assert!(!titles.iter().any(|t| t == "rust weekly")); // read -> excluded
    assert!(!titles.iter().any(|t| t == "go")); // no "rust" -> excluded
}

// `NOT (NULL LIKE ...)` is NULL; the COALESCE wrapper keeps negation
// including NULL-column rows.
#[tokio::test]
async fn query_negated_author_includes_null_author_entries() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("jane's post"),
            author: Some("jane"),
            ..EntryUpsert::new(feed_id, "guid-jane")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("bob's post"),
            author: Some("bob"),
            ..EntryUpsert::new(feed_id, "guid-bob")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            feed_id,
            guid: "guid-anon",
            title: Some("anonymous post"),
            link: None,
            content: None,
            summary: None,
            author: None, // no author (NULL)
            published_at: None,
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        query: Some(query::parse("-author:jane").unwrap()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    let titles: Vec<_> = results
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(titles.iter().any(|t| t == "bob's post"));
    assert!(titles.iter().any(|t| t == "anonymous post")); // NULL author -> included
    assert!(!titles.iter().any(|t| t == "jane's post"));
}

#[tokio::test]
async fn query_feed_name_fuzzy_match() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let rust_feed_id = feed::create_feed(
        &db,
        &feed::CreateFeedParams {
            category_id,
            url: "https://example.com/rust-blog.xml",
            title: Some("Rust Blog"),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id;
    let other_feed_id = create_test_feed(&db, category_id, "https://example.com/other.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Post from Rust Blog"),
            ..EntryUpsert::new(rust_feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Post from other feed"),
            ..EntryUpsert::new(other_feed_id, "guid-2")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        query: Some(query::parse("feed:rust").unwrap()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert!(
        results
            .iter()
            .all(|r| r.entry.title.as_deref() == Some("Post from Rust Blog"))
    );
}

#[tokio::test]
async fn query_after_date_filters() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("old"),
            published_at: Some(chrono::Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap()),
            ..EntryUpsert::new(feed_id, "guid-old")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("new"),
            published_at: Some(chrono::Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap()),
            ..EntryUpsert::new(feed_id, "guid-new")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        query: Some(query::parse("after:2026-01-01").unwrap()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    let titles: Vec<_> = results
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(titles.iter().any(|t| t == "new"));
    assert!(!titles.iter().any(|t| t == "old"));
}

#[tokio::test]
async fn query_escapes_literal_percent() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("50% off"),
            ..EntryUpsert::new(feed_id, "guid-off")
        },
    )
    .await
    .unwrap();
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("50 dollars"),
            ..EntryUpsert::new(feed_id, "guid-dollars")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        query: Some(query::parse("\"50%\"").unwrap()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    let titles: Vec<_> = results
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(titles.iter().any(|t| t == "50% off"));
    assert!(!titles.iter().any(|t| t == "50 dollars")); // literal %, not wildcard
}

#[tokio::test]
async fn search_matches_plain_text_across_tags_not_attributes() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    // (a) term split across inline tags — must match via content_text.
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("x"),
            content: Some("超<b>少女</b>登場"),
            ..EntryUpsert::new(feed_id, "a")
        },
    )
    .await
    .unwrap();
    // (b) term only inside an href attribute — must NOT match.
    upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("y"),
            content: Some(r#"<a href="/超少女">z</a>"#),
            ..EntryUpsert::new(feed_id, "b")
        },
    )
    .await
    .unwrap();

    let filter = EntryFilter {
        search: Some("超少女".to_string()),
        ..Default::default()
    };
    let results = list_by_user(&db, &page(user_id, &filter, 50, 0))
        .await
        .unwrap();
    let guids: Vec<_> = results.iter().map(|r| r.entry.guid.clone()).collect();
    assert!(
        guids.contains(&"a".to_string()),
        "tag-split term should match"
    );
    assert!(
        !guids.contains(&"b".to_string()),
        "attribute-only term should not match"
    );
}

#[tokio::test]
async fn test_mark_read_by_ids() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let user2_id = create_test_user(&db, "testuser2").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let category2_id = create_test_category(&db, user2_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let feed2_id = create_test_feed(&db, category2_id, "https://example2.com/feed.xml").await;

    let (entry1, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Entry 1"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    let (entry2, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Entry 2"),
            ..EntryUpsert::new(feed_id, "guid-2")
        },
    )
    .await
    .unwrap();
    let (entry3, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Entry 3"),
            ..EntryUpsert::new(feed_id, "guid-3")
        },
    )
    .await
    .unwrap();

    let (other_entry, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("Other Entry"),
            ..EntryUpsert::new(feed2_id, "guid-4")
        },
    )
    .await
    .unwrap();

    // All entries should be unread
    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 3);
    assert_eq!(count_unread_by_user(&db, user2_id).await.unwrap(), 1);

    let marked = mark_read_by_ids(&db, user_id, &[entry1.id, entry2.id])
        .await
        .unwrap();
    assert_eq!(marked, 2);

    // Entry 3 should still be unread
    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 1);

    // Try to mark user 2's entry as read with user 1's credentials - should not work
    let marked = mark_read_by_ids(&db, user_id, &[other_entry.id])
        .await
        .unwrap();
    assert_eq!(marked, 0);

    // User 2's entry should still be unread
    assert_eq!(count_unread_by_user(&db, user2_id).await.unwrap(), 1);

    let marked = mark_read_by_ids(&db, user_id, &[entry1.id, entry2.id])
        .await
        .unwrap();
    assert_eq!(marked, 0);

    // Empty array should return 0
    let marked = mark_read_by_ids(&db, user_id, &[]).await.unwrap();
    assert_eq!(marked, 0);

    let marked = mark_read_by_ids(&db, user_id, &[entry3.id]).await.unwrap();
    assert_eq!(marked, 1);

    // All user 1 entries should now be read
    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 0);
}

#[tokio::test]
async fn test_upsert_entry_id_returns_id_and_is_new() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let first = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("Title"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    let id1 = match first {
        UpsertOutcome::Inserted(id) => id,
        o => panic!("expected Inserted, got {o:?}"),
    };
    let row = find_by_id(&db, id1).await.unwrap().unwrap();
    assert_eq!(row.title.as_deref(), Some("Title"));

    let second = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("Title 2"),
            ..EntryUpsert::new(feed_id, "guid-1")
        },
    )
    .await
    .unwrap();
    assert!(matches!(second, UpsertOutcome::Updated(id) if id == id1));
    let row = find_by_id(&db, id1).await.unwrap().unwrap();
    assert_eq!(row.title.as_deref(), Some("Title 2"));
}

/// Re-upserting identical values reports `Unchanged` and leaves `updated_at` untouched.
#[tokio::test]
async fn upsert_identical_values_is_unchanged_and_does_not_write() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let upsert = EntryUpsert {
        title: Some("Title"),
        link: Some("https://example.com/a"),
        content: Some("<p>body</p>"),
        summary: Some("summary"),
        author: Some("author"),
        ..EntryUpsert::new(feed_id, "guid-1")
    };
    let first = upsert_entry_id(&db, &upsert).await.unwrap();
    let id = match first {
        UpsertOutcome::Inserted(id) => id,
        o => panic!("expected Inserted, got {o:?}"),
    };
    let before = find_by_id(&db, id).await.unwrap().unwrap();

    // Same values again: no write, no bump of `updated_at`.
    let second = upsert_entry_id(&db, &upsert).await.unwrap();
    assert!(
        matches!(second, UpsertOutcome::Unchanged(uid) if uid == id),
        "expected Unchanged, got {second:?}"
    );
    let after = find_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(before.updated_at, after.updated_at);

    // A single differing column still updates.
    let third = upsert_entry_id(
        &db,
        &EntryUpsert {
            content: Some("<p>edited</p>"),
            ..upsert
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(third, UpsertOutcome::Updated(uid) if uid == id),
        "expected Updated, got {third:?}"
    );
    let edited = find_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(edited.content.as_deref(), Some("<p>edited</p>"));
}

/// NULL staying NULL is not a difference: the guard must be NULL-safe, not `<>`.
#[tokio::test]
async fn upsert_null_columns_compare_null_safely() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let first = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("T"),
            ..EntryUpsert::new(feed_id, "guid-n")
        },
    )
    .await
    .unwrap();
    let id = match first {
        UpsertOutcome::Inserted(id) => id,
        o => panic!("expected Inserted, got {o:?}"),
    };

    let second = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("T"),
            ..EntryUpsert::new(feed_id, "guid-n")
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(second, UpsertOutcome::Unchanged(uid) if uid == id),
        "all-NULL columns unchanged should be Unchanged, got {second:?}"
    );

    // NULL -> non-NULL is a real change.
    let third = upsert_entry_id(
        &db,
        &EntryUpsert {
            feed_id,
            guid: "guid-n",
            title: Some("T"),
            link: Some("https://example.com/x"),
            content: None,
            summary: None,
            author: None,
            published_at: None,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(third, UpsertOutcome::Updated(uid) if uid == id),
        "NULL -> value should be Updated, got {third:?}"
    );

    // non-NULL -> NULL is a real change too.
    let fourth = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("T"),
            ..EntryUpsert::new(feed_id, "guid-n")
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(fourth, UpsertOutcome::Updated(uid) if uid == id),
        "value -> NULL should be Updated, got {fourth:?}"
    );
}

#[tokio::test]
async fn upsert_populates_content_text_stripped() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let out = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("t"),
            content: Some("超<b>少女</b>與機器人"),
            ..EntryUpsert::new(feed_id, "g1")
        },
    )
    .await
    .unwrap();
    let id = match out {
        UpsertOutcome::Inserted(id) => id,
        o => panic!("expected Inserted, got {o:?}"),
    };
    let ct = crate::query_scalar!(
        &db,
        Option<String>,
        "SELECT content_text FROM entry WHERE id = $1",
        id
    )
    .unwrap();
    assert_eq!(ct.as_deref(), Some("超少女與機器人"));
}

#[tokio::test]
async fn test_upsert_skips_tombstoned_guid() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    insert_tombstone(&db, feed_id, "ghost").await.unwrap();
    let outcome = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("Ghost"),
            ..EntryUpsert::new(feed_id, "ghost")
        },
    )
    .await
    .unwrap();
    assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));
    assert!(
        find_by_guid_and_feed(&db, "ghost", feed_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_upsert_inserts_then_updates() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let first = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("First"),
            ..EntryUpsert::new(feed_id, "g1")
        },
    )
    .await
    .unwrap();
    let id = match first {
        UpsertOutcome::Inserted(id) => id,
        other => panic!("expected Inserted, got {other:?}"),
    };

    let second = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("Updated"),
            ..EntryUpsert::new(feed_id, "g1")
        },
    )
    .await
    .unwrap();
    assert!(matches!(second, UpsertOutcome::Updated(uid) if uid == id));
}

#[tokio::test]
async fn test_star_unstar_and_mark_unread_by_ids_tx() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let user2_id = create_test_user(&db, "testuser2").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let category2_id = create_test_category(&db, user2_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
    let feed2_id = create_test_feed(&db, category2_id, "https://example2.com/feed.xml").await;

    let (e1, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("E1"),
            ..EntryUpsert::new(feed_id, "g1")
        },
    )
    .await
    .unwrap();
    let (e2, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("E2"),
            ..EntryUpsert::new(feed_id, "g2")
        },
    )
    .await
    .unwrap();
    let (other, _) = upsert_entry(
        &db,
        &EntryUpsert {
            title: Some("E3"),
            ..EntryUpsert::new(feed2_id, "g3")
        },
    )
    .await
    .unwrap();

    // Commit before each read: the one-connection pool would be held by a tx.
    macro_rules! in_tx {
        ($db:expr, $f:ident($($arg:expr),*)) => {{
            let mut tx = $db.begin().await.unwrap();
            let n = $f(&mut tx, $($arg),*).await.unwrap();
            tx.commit().await.unwrap();
            n
        }};
    }

    // Star e1, e2 — only currently-unstarred rows count
    let starred = in_tx!(db, star_by_ids_tx(user_id, &[e1.id, e2.id]));
    assert_eq!(starred, 2);
    assert!(
        find_by_id(&db, e1.id)
            .await
            .unwrap()
            .unwrap()
            .starred_at
            .is_some()
    );

    // Starring again is a no-op (already starred)
    assert_eq!(in_tx!(db, star_by_ids_tx(user_id, &[e1.id, e2.id])), 0);

    // Ownership scope: cannot star another user's entry
    assert_eq!(in_tx!(db, star_by_ids_tx(user_id, &[other.id])), 0);
    assert!(
        find_by_id(&db, other.id)
            .await
            .unwrap()
            .unwrap()
            .starred_at
            .is_none()
    );

    // Unstar e1
    assert_eq!(in_tx!(db, unstar_by_ids_tx(user_id, &[e1.id])), 1);
    assert!(
        find_by_id(&db, e1.id)
            .await
            .unwrap()
            .unwrap()
            .starred_at
            .is_none()
    );

    assert_eq!(
        mark_read_by_ids(&db, user_id, &[e1.id, e2.id])
            .await
            .unwrap(),
        2
    );
    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 0);
    let unread = in_tx!(db, mark_unread_by_ids_tx(user_id, &[e1.id, e2.id]));
    assert_eq!(unread, 2);
    assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 2);

    // Empty input is a no-op
    assert_eq!(in_tx!(db, star_by_ids_tx(user_id, &[])), 0);
    assert_eq!(in_tx!(db, mark_unread_by_ids_tx(user_id, &[])), 0);
}

#[tokio::test]
async fn test_find_position_counts_within_filter() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let mut entries = Vec::new();
    for i in 0..5 {
        let published = Utc::now() + chrono::Duration::seconds(i * 10);
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                published_at: Some(published),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        entries.push(entry);
    }

    // Newest first: entry 4 is #1, entry 0 is #5.
    let all = EntryFilter::default();
    let newest = find_position(&db, user_id, entries[4].id, &all)
        .await
        .unwrap();
    assert_eq!((newest.position, newest.total), (1, 5));
    let oldest = find_position(&db, user_id, entries[0].id, &all)
        .await
        .unwrap();
    assert_eq!((oldest.position, oldest.total), (5, 5));

    // Starring 1 and 3 shrinks the list to two; entry 1 is then #2 of 2.
    star_entry(&db, entries[1].id).await.unwrap();
    star_entry(&db, entries[3].id).await.unwrap();
    let starred = EntryFilter {
        starred_only: true,
        ..Default::default()
    };
    let second = find_position(&db, user_id, entries[1].id, &starred)
        .await
        .unwrap();
    assert_eq!((second.position, second.total), (2, 2));
}

#[tokio::test]
async fn test_find_position_stops_counting_at_the_limit() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let n = POSITION_COUNT_LIMIT + 3;
    let mut ids = Vec::new();
    for i in 0..n {
        let published = Utc::now() + chrono::Duration::seconds(i);
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                published_at: Some(published),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        ids.push(entry.id);
    }

    let all = EntryFilter::default();
    let newest = find_position(&db, user_id, ids[ids.len() - 1], &all)
        .await
        .unwrap();
    assert_eq!(newest.position, 1);
    // Past the limit the counts saturate at `limit + 1`.
    assert_eq!(newest.total, POSITION_COUNT_LIMIT + 1);
    let oldest = find_position(&db, user_id, ids[0], &all).await.unwrap();
    assert_eq!(oldest.position, POSITION_COUNT_LIMIT + 1);
    assert_eq!(oldest.limit, POSITION_COUNT_LIMIT);
}

#[tokio::test]
async fn test_find_position_unread_snapshot_counts_entries_read_since() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    // entries[4] is newest in list order.
    let mut entries = Vec::new();
    for i in 0..5 {
        let published = Utc::now() + chrono::Duration::seconds(i * 10);
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                published_at: Some(published),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        entries.push(entry);
    }
    // entries[1] was read before the snapshot, entries[3] after it.
    crate::db_execute!(
        &db,
        "UPDATE entry SET read_at = datetime('now', '-1 hour') WHERE id = $1",
        entries[1].id
    )
    .unwrap();
    crate::db_execute!(
        &db,
        "UPDATE entry SET read_at = datetime('now') WHERE id = $1",
        entries[3].id
    )
    .unwrap();
    let snapshot = (Utc::now() - chrono::Duration::minutes(10))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let filter = EntryFilter {
        unread_only: true,
        read_after: Some(snapshot),
        ..Default::default()
    };

    // The list is entries 4, 3, 2, 0: entries[1] is gone, entries[3] stays.
    let newest = find_position(&db, user_id, entries[4].id, &filter)
        .await
        .unwrap();
    assert_eq!((newest.position, newest.total), (1, 4));
    let read_since = find_position(&db, user_id, entries[3].id, &filter)
        .await
        .unwrap();
    assert_eq!((read_since.position, read_since.total), (2, 4));
    let oldest = find_position(&db, user_id, entries[0].id, &filter)
        .await
        .unwrap();
    assert_eq!((oldest.position, oldest.total), (4, 4));
}

/// 20 feeds and 3000 entries, 85% of them read, then `ANALYZE`. The planner
/// picks between the unread indexes on statistics, so an empty schema says
/// nothing about which one a real list gets.
async fn seed_mostly_read_entries(db: &Db) {
    let user_id = create_test_user(db, "u").await;
    let category_id = create_test_category(db, user_id, "Tech").await;
    for i in 0..20 {
        create_test_feed(db, category_id, &format!("https://example.com/{i}.xml")).await;
    }
    crate::db_execute!(
        db,
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 3000) \
         INSERT INTO entry(feed_id, guid, published_at, read_at, created_at) \
         SELECT (SELECT MIN(id) FROM feed) + (i % 20), 'g' || i, \
                strftime('%Y-%m-%dT%H:%M:%SZ', 1700000000 + i * 300, 'unixepoch'), \
                CASE WHEN (i * 7919) % 100 < 85 THEN '2024-01-01 00:00:00' END, \
                strftime('%Y-%m-%d %H:%M:%S', 1700000000 + i * 300, 'unixepoch') \
         FROM n"
    )
    .unwrap();
    crate::db_execute!(db, "ANALYZE").unwrap();
}

/// Plan of one position count over one of `filter`'s slices.
async fn position_count_plan(db: &Db, filter: &EntryFilter, slice: usize) -> String {
    let counter = PositionCounter {
        user_id: 1,
        entry_id: 1,
        dialect: Dialect::Sqlite,
        slices: count_slices(filter),
    };
    let range = " AND COALESCE(e.published_at, e.created_at) > \
                  (SELECT COALESCE(s.published_at, s.created_at) FROM entry s WHERE s.id = $2)";
    let index = " INDEXED BY idx_entry_unread_feed_sort";
    let (sql, binds) = counter.slice_sql(range, &counter.slices[slice], index);
    explain_plan_for(db, &sql, binds.len()).await
}

/// Every pinned index must be usable for its query: SQLite rejects an
/// `INDEXED BY` it cannot use at prepare time, which no plan check would show.
#[tokio::test]
async fn find_position_runs_with_pinned_unread_indexes() {
    let db = setup_db().await;
    seed_mostly_read_entries(&db).await;
    let user_id = crate::query_scalar!(&db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let entry_id =
        crate::query_scalar!(&db, i64, "SELECT MIN(id) FROM entry WHERE read_at IS NULL").unwrap();
    let strict = EntryFilter {
        unread_only: true,
        ..Default::default()
    };
    // After the seeded `read_at`, so no read entry counts as read since.
    let snapshot = EntryFilter {
        read_after: Some("2024-06-01 00:00:00".to_string()),
        ..strict.clone()
    };
    for filter in [&strict, &snapshot] {
        let position = find_position(&db, user_id, entry_id, filter).await.unwrap();
        assert_eq!(position.total, 450, "{filter:?}");
        assert_eq!(position.position, 450, "{filter:?}");
    }
}

/// The newer-than count pins `idx_entry_unread_feed_sort`, so it needs the
/// migration: without it `feed_id` is read off the table row for every entry
/// counted (1035 page misses against 27 on 20k entries). The plan only shows
/// the pin took effect; the page misses come from `cargo bench --bench entry_pages`.
#[tokio::test]
async fn position_unread_count_is_index_covered() {
    let db = setup_db().await;
    seed_mostly_read_entries(&db).await;
    let filter = EntryFilter {
        unread_only: true,
        ..Default::default()
    };
    let plan = position_count_plan(&db, &filter, 0).await;
    assert!(
        plan.contains("idx_entry_unread_feed_sort"),
        "unread position count must use idx_entry_unread_feed_sort: {plan}"
    );
}

/// The snapshot predicate as one `OR` is O(table) (about 12k page misses on
/// 20k entries), so each half is counted on its own index.
#[tokio::test]
async fn position_unread_snapshot_count_is_split_into_indexed_halves() {
    let db = setup_db().await;
    seed_mostly_read_entries(&db).await;
    let filter = EntryFilter {
        unread_only: true,
        read_after: Some("2024-01-01 00:00:00".to_string()),
        ..Default::default()
    };
    let unread = position_count_plan(&db, &filter, 0).await;
    assert!(
        unread.contains("idx_entry_unread_feed_sort"),
        "unread half must use idx_entry_unread_feed_sort: {unread}"
    );
    let read_since = position_count_plan(&db, &filter, 1).await;
    assert!(
        read_since.contains("idx_entry_read_at"),
        "read-since half must use idx_entry_read_at: {read_since}"
    );
}

/// PostgreSQL's `CROSS JOIN` takes no `ON`, and `INDEXED BY` does not exist
/// there; no test here runs on PostgreSQL, so check the generated text.
#[test]
fn position_count_sql_is_valid_postgres() {
    let filter = EntryFilter {
        unread_only: true,
        read_after: Some("2024-01-01 00:00:00".to_string()),
        ..Default::default()
    };
    let counter = PositionCounter {
        user_id: 1,
        entry_id: 1,
        dialect: Dialect::Postgres,
        slices: count_slices(&filter),
    };
    for slice in &counter.slices {
        let (sql, _) = counter.slice_sql(" AND e.id > $2", slice, " INDEXED BY idx_x");
        assert!(!sql.contains("CROSS JOIN"), "{sql}");
        assert!(!sql.contains("INDEXED BY"), "{sql}");
    }
}

/// Default-order [`EntryListQuery`] for the list tests.
fn page(user_id: i64, filter: &EntryFilter, limit: i64, offset: i64) -> EntryListQuery<'_> {
    EntryListQuery {
        user_id,
        filter,
        sort_order: EntrySortOrder::default(),
        limit,
        offset,
    }
}

#[tokio::test]
async fn test_find_neighbors_starred_only() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let mut entries = Vec::new();
    for i in 0..5 {
        let published = Utc::now() + chrono::Duration::seconds(i * 10);
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                published_at: Some(published),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        entries.push(entry);
    }

    // Star entries 1 and 3 (0-indexed)
    star_entry(&db, entries[1].id).await.unwrap();
    star_entry(&db, entries[3].id).await.unwrap();

    let filter = EntryFilter {
        starred_only: true,
        ..Default::default()
    };
    let neighbors = find_neighbors(&db, user_id, entries[3].id, &filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, None); // no starred entry newer than entry 3
    assert_eq!(neighbors.next_id, Some(entries[1].id)); // entry 1 is older and starred

    // From entry 1 (starred):
    // prev (newer) = entry 3 (starred, newer)
    // next (older) = none
    let neighbors = find_neighbors(&db, user_id, entries[1].id, &filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, Some(entries[3].id));
    assert_eq!(neighbors.next_id, None);

    // Without filter, entry 3 should see entry 4 as prev and entry 2 as next
    let no_filter = EntryFilter::default();
    let neighbors = find_neighbors(&db, user_id, entries[3].id, &no_filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, Some(entries[4].id));
    assert_eq!(neighbors.next_id, Some(entries[2].id));
}

#[tokio::test]
async fn test_find_neighbors_unread_only_read_after() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    // entries[4] is newest in list order.
    let mut entries = Vec::new();
    for i in 0..5 {
        let published = Utc::now() + chrono::Duration::seconds(i * 10);
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                published_at: Some(published),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        entries.push(entry);
    }

    // entries[1]: read an hour ago — before the snapshot boundary.
    crate::db_execute!(
        &db,
        "UPDATE entry SET read_at = datetime('now', '-1 hour') WHERE id = $1",
        entries[1].id
    )
    .unwrap();
    // entries[2]: read just now — inside the snapshot.
    crate::db_execute!(
        &db,
        "UPDATE entry SET read_at = datetime('now') WHERE id = $1",
        entries[2].id
    )
    .unwrap();

    // Snapshot boundary: 10 minutes ago.
    let snapshot = (Utc::now() - chrono::Duration::minutes(10))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let filter = EntryFilter {
        unread_only: true,
        read_after: Some(snapshot),
        ..Default::default()
    };

    // From entries[3]: next (older) lands on the just-read entries[2]
    // — still reachable inside the snapshot.
    let neighbors = find_neighbors(&db, user_id, entries[3].id, &filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, Some(entries[4].id));
    assert_eq!(neighbors.next_id, Some(entries[2].id));

    // From entries[2]: prev is the unread entries[3]; next skips the
    // pre-snapshot entries[1] and lands on the unread entries[0].
    let neighbors = find_neighbors(&db, user_id, entries[2].id, &filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, Some(entries[3].id));
    assert_eq!(neighbors.next_id, Some(entries[0].id));

    // Plain unread_only without read_after keeps the strict live
    // filter: both read entries are skipped.
    let strict = EntryFilter {
        unread_only: true,
        ..Default::default()
    };
    let neighbors = find_neighbors(&db, user_id, entries[3].id, &strict)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, Some(entries[4].id));
    assert_eq!(neighbors.next_id, Some(entries[0].id));
}

#[tokio::test]
async fn test_find_neighbors_read_only() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "testuser").await;
    let category_id = create_test_category(&db, user_id, "Tech").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

    let mut entries = Vec::new();
    for i in 0..4 {
        let published = Utc::now() + chrono::Duration::seconds(i * 10);
        let (entry, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("Entry {i}")),
                published_at: Some(published),
                ..EntryUpsert::new(feed_id, &format!("guid-{i}"))
            },
        )
        .await
        .unwrap();
        entries.push(entry);
    }

    mark_as_read(&db, entries[0].id).await.unwrap();
    mark_as_read(&db, entries[2].id).await.unwrap();

    // From entry 2 (read, published_at = now+20s), with read_only filter:
    // prev (newer) = none (entry 3 is newer but unread)
    // next (older) = entry 0 (read, older)
    let filter = EntryFilter {
        read_only: true,
        ..Default::default()
    };
    let neighbors = find_neighbors(&db, user_id, entries[2].id, &filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, None);
    assert_eq!(neighbors.next_id, Some(entries[0].id));

    // From entry 0 (read, published_at = now+0s):
    // prev (newer) = entry 2 (read, newer)
    // next (older) = none
    let neighbors = find_neighbors(&db, user_id, entries[0].id, &filter)
        .await
        .unwrap();
    assert_eq!(neighbors.prev_id, Some(entries[2].id));
    assert_eq!(neighbors.next_id, None);
}

#[test]
fn cursor_parses_composite_format() {
    let c = ContinuationCursor::parse("2026-04-26 12:34:56|142").expect("composite parses");
    match c {
        ContinuationCursor::Composite { sort_ts, id } => {
            assert_eq!(sort_ts, "2026-04-26 12:34:56");
            assert_eq!(id, 142);
        }
        ContinuationCursor::LegacyId(_) => panic!("expected Composite"),
    }
}

#[test]
fn cursor_parses_bare_i64_as_legacy() {
    let c = ContinuationCursor::parse("142").expect("legacy parses");
    match c {
        ContinuationCursor::LegacyId(id) => assert_eq!(id, 142),
        ContinuationCursor::Composite { .. } => panic!("expected LegacyId"),
    }
}

#[test]
fn cursor_rejects_garbage() {
    assert!(ContinuationCursor::parse("not-a-cursor").is_none());
    assert!(ContinuationCursor::parse("|").is_none());
    assert!(ContinuationCursor::parse("ts|notnum").is_none());
    assert!(ContinuationCursor::parse("").is_none());
}

#[test]
fn cursor_encodes_composite() {
    assert_eq!(
        ContinuationCursor::encode_composite("2026-04-26 12:34:56", 142),
        "2026-04-26 12:34:56|142"
    );
}

#[tokio::test]
async fn fetch_sort_ts_returns_published_or_created_for_publishedat() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

    // Entry with published_at set
    let id1 = crate::query_scalar!(
        &db,
        i64,
        "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3) RETURNING id",
        feed_id,
        "g1",
        "2026-04-01 10:00:00"
    )
    .unwrap();

    // Entry with published_at NULL → COALESCE falls back to created_at
    let id2 = crate::query_scalar!(
        &db,
        i64,
        "INSERT INTO entry (feed_id, guid, created_at) VALUES ($1, $2, $3) RETURNING id",
        feed_id,
        "g2",
        "2026-04-02 11:00:00"
    )
    .unwrap();

    let ts1 = fetch_sort_ts(&db, id1, EntrySortOrder::PublishedAt)
        .await
        .unwrap();
    let ts2 = fetch_sort_ts(&db, id2, EntrySortOrder::PublishedAt)
        .await
        .unwrap();
    assert_eq!(ts1.as_deref(), Some("2026-04-01 10:00:00"));
    assert_eq!(ts2.as_deref(), Some("2026-04-02 11:00:00"));
}

#[tokio::test]
async fn fetch_sort_ts_returns_read_at_for_readat_sort() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

    let id = crate::query_scalar!(
        &db,
        i64,
        "INSERT INTO entry (feed_id, guid, read_at) VALUES ($1, $2, $3) RETURNING id",
        feed_id,
        "g1",
        "2026-04-03 12:00:00"
    )
    .unwrap();

    let ts = fetch_sort_ts(&db, id, EntrySortOrder::ReadAt)
        .await
        .unwrap();
    assert_eq!(ts.as_deref(), Some("2026-04-03 12:00:00"));
}

/// The in-query `sort_ts` must be byte-identical to [`fetch_sort_ts`], or a
/// cursor built from it would skip or repeat rows at the page boundary.
#[tokio::test]
async fn listed_sort_ts_matches_fetch_sort_ts_for_every_order() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;
    for (guid, published) in [("g1", Some("2026-04-01 08:00:00")), ("g2", None)] {
        crate::db_execute!(
            &db,
            "INSERT INTO entry (feed_id, guid, published_at, read_at, starred_at) \
             VALUES ($1, $2, $3, $4, $5)",
            feed_id,
            guid,
            published,
            "2026-04-03 12:00:00",
            "2026-04-02 09:30:00"
        )
        .unwrap();
    }

    for sort_order in [
        EntrySortOrder::PublishedAt,
        EntrySortOrder::ReadAt,
        EntrySortOrder::StarredAt,
    ] {
        let params = ContinuationParams {
            limit: 10,
            sort_order,
            ..Default::default()
        };
        let rows =
            list_by_user_with_continuation_ts(&db, user_id, &EntryFilter::default(), &params)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
        for (e, ts) in &rows {
            let expected = fetch_sort_ts(&db, e.entry.id, sort_order).await.unwrap();
            assert!(ts.is_some(), "{sort_order:?}");
            assert_eq!(*ts, expected, "{sort_order:?}");
        }
    }
}

#[test]
fn next_continuation_uses_the_last_kept_row() {
    let rows = [
        (1_i64, Some("t1".to_string())),
        (2, Some("t2".to_string())),
        (3, None),
    ];
    assert_eq!(
        next_continuation(&rows, 2, |id| *id).as_deref(),
        Some("t2|2")
    );
    // No look-ahead row: no next page.
    assert_eq!(next_continuation(&rows, 3, |id| *id), None);
    assert_eq!(next_continuation(&rows[..0], 0, |id| *id), None);
}

#[tokio::test]
async fn fetch_sort_ts_returns_none_for_missing_id() {
    let db = setup_db().await;
    let ts = fetch_sort_ts(&db, 99999, EntrySortOrder::PublishedAt)
        .await
        .unwrap();
    assert_eq!(ts, None);
}

#[tokio::test]
async fn composite_cursor_walks_non_monotonic_data_without_skip() {
    // #164: when id and published_at order diverge, the legacy `e.id < ?`
    // cursor skips entries; the composite cursor must not.
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

    // 6 monotonic entries, then 4 back-dated ones (new ids, old timestamps).
    let monotonic = [
        ("g1", "2026-04-01 10:00:00"),
        ("g2", "2026-04-02 10:00:00"),
        ("g3", "2026-04-03 10:00:00"),
        ("g4", "2026-04-04 10:00:00"),
        ("g5", "2026-04-05 10:00:00"),
        ("g6", "2026-04-06 10:00:00"),
    ];
    let backdated = [
        ("g7-bd", "2026-03-01 10:00:00"),
        ("g8-bd", "2026-03-02 10:00:00"),
        ("g9-bd", "2026-03-03 10:00:00"),
        ("g10-bd", "2026-03-04 10:00:00"),
    ];
    for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
        crate::db_execute!(
            &db,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
            feed_id,
            *guid,
            *ts
        )
        .unwrap();
    }

    let filter = EntryFilter::default();
    let mut seen: Vec<i64> = Vec::new();
    let mut cursor: Option<ContinuationCursor> = None;
    let page_limit: i64 = 3;

    // Walk pages (DESC sort by COALESCE(published_at, created_at)) until empty
    loop {
        let pagination = ContinuationParams {
            oldest_first: false,
            limit: page_limit,
            continuation: cursor.clone(),
            ot: None,
            nt: None,
            sort_order: EntrySortOrder::PublishedAt,
        };
        let page = list_by_user_with_continuation(&db, user_id, &filter, &pagination)
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        for entry_with_feed in &page {
            assert!(
                !seen.contains(&entry_with_feed.entry.id),
                "duplicate id {}",
                entry_with_feed.entry.id
            );
            seen.push(entry_with_feed.entry.id);
        }
        let last = page.last().unwrap();
        let sort_ts = fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
            .await
            .unwrap()
            .unwrap();
        cursor = Some(ContinuationCursor::Composite {
            sort_ts,
            id: last.entry.id,
        });
        // safety: don't loop forever
        assert!(seen.len() <= 100, "runaway loop");
    }

    assert_eq!(
        seen.len(),
        10,
        "must visit all 10 entries; saw {}",
        seen.len()
    );
}

#[tokio::test]
async fn composite_cursor_walks_non_monotonic_data_oldest_first_without_skip() {
    // As above, but the oldest_first (ASC) path, used by GReader `r=o`.
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

    let monotonic = [
        ("g1", "2026-04-01 10:00:00"),
        ("g2", "2026-04-02 10:00:00"),
        ("g3", "2026-04-03 10:00:00"),
        ("g4", "2026-04-04 10:00:00"),
        ("g5", "2026-04-05 10:00:00"),
        ("g6", "2026-04-06 10:00:00"),
    ];
    let backdated = [
        ("g7-bd", "2026-03-01 10:00:00"),
        ("g8-bd", "2026-03-02 10:00:00"),
        ("g9-bd", "2026-03-03 10:00:00"),
        ("g10-bd", "2026-03-04 10:00:00"),
    ];
    for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
        crate::db_execute!(
            &db,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
            feed_id,
            *guid,
            *ts
        )
        .unwrap();
    }

    let filter = EntryFilter::default();
    let mut seen: Vec<i64> = Vec::new();
    let mut cursor: Option<ContinuationCursor> = None;
    let page_limit: i64 = 3;

    loop {
        let pagination = ContinuationParams {
            oldest_first: true,
            limit: page_limit,
            continuation: cursor.clone(),
            ot: None,
            nt: None,
            sort_order: EntrySortOrder::PublishedAt,
        };
        let page = list_by_user_with_continuation(&db, user_id, &filter, &pagination)
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        for entry_with_feed in &page {
            assert!(
                !seen.contains(&entry_with_feed.entry.id),
                "duplicate id {}",
                entry_with_feed.entry.id
            );
            seen.push(entry_with_feed.entry.id);
        }
        let last = page.last().unwrap();
        let sort_ts = fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
            .await
            .unwrap()
            .unwrap();
        cursor = Some(ContinuationCursor::Composite {
            sort_ts,
            id: last.entry.id,
        });
        assert!(seen.len() <= 100, "runaway loop");
    }

    assert_eq!(
        seen.len(),
        10,
        "must visit all 10 entries on ASC walk; saw {}",
        seen.len()
    );
}

#[tokio::test]
async fn legacy_bare_i64_cursor_still_paginates() {
    // Legacy `e.id < ?` cursors still work on monotonic data.
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

    for i in 1..=5 {
        crate::db_execute!(
            &db,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
            feed_id,
            format!("g{}", i),
            format!("2026-04-0{} 10:00:00", i)
        )
        .unwrap();
    }

    let max_id = crate::query_scalar!(&db, i64, "SELECT MAX(id) FROM entry").unwrap();

    let pagination = ContinuationParams {
        oldest_first: false,
        limit: 10,
        continuation: Some(ContinuationCursor::LegacyId(max_id)),
        ot: None,
        nt: None,
        sort_order: EntrySortOrder::PublishedAt,
    };
    let page = list_by_user_with_continuation(&db, user_id, &EntryFilter::default(), &pagination)
        .await
        .unwrap();

    // 4 entries below the boundary id
    assert_eq!(page.len(), 4);
    for entry_with_feed in &page {
        assert!(entry_with_feed.entry.id < max_id);
    }
}

#[test]
fn is_no_entry_side_predicate_matches_only_user_only_filter() {
    // Default filter (used by /entries) has no entry-side predicate.
    assert!(is_no_entry_side_predicate(&EntryFilter::default()));

    // Any of these flags pulls in an entry-side predicate.
    let cases = [
        EntryFilter {
            feed_id: Some(1),
            ..Default::default()
        },
        EntryFilter {
            category_id: Some(1),
            ..Default::default()
        },
        EntryFilter {
            unread_only: true,
            ..Default::default()
        },
        EntryFilter {
            starred_only: true,
            ..Default::default()
        },
        EntryFilter {
            read_only: true,
            ..Default::default()
        },
        EntryFilter {
            search: Some("foo".into()),
            ..Default::default()
        },
        EntryFilter {
            has_summary: Some(true),
            ..Default::default()
        },
        EntryFilter {
            has_summary: Some(false),
            ..Default::default()
        },
    ];
    for f in &cases {
        assert!(
            !is_no_entry_side_predicate(f),
            "expected entry-side predicate for filter: {f:?}"
        );
    }
}

/// Concatenated EXPLAIN QUERY PLAN `detail`s; binds `n_params` dummy `i64`s.
async fn explain_plan_for(db: &Db, sql: &str, n_params: usize) -> String {
    let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
    let rows: Vec<(i64, i64, i64, String)> = match db.inner() {
        DbInner::Sqlite(pool) => {
            let mut q = sqlx::query_as::<sqlx::Sqlite, (i64, i64, i64, String)>(
                sqlx::AssertSqlSafe(explain_sql),
            );
            for _ in 0..n_params {
                q = q.bind(1i64);
            }
            q.fetch_all(pool).await.unwrap()
        }
        DbInner::Postgres(_) => unreachable!("tests run against sqlite"),
    };
    rows.into_iter()
        .map(|r| r.3)
        .collect::<Vec<_>>()
        .join(" | ")
}

#[tokio::test]
async fn list_by_user_uses_partial_index_for_starred() {
    let db = setup_db().await;
    let _ = create_test_user(&db, "u").await;
    // INDEXED BY is mandatory, so a tiny dataset suffices.
    let sql = r"
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.full_content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, f.site_url, c.id, c.name,
               CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon,
               f.custom_referrer
        FROM entry e INDEXED BY idx_entry_starred_sort
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE c.user_id = $1 AND e.starred_at IS NOT NULL
        ORDER BY COALESCE(e.published_at, e.created_at) DESC
        LIMIT 51
    ";
    let plan = explain_plan_for(&db, sql, 1).await;
    assert!(
        plan.contains("idx_entry_starred_sort"),
        "plan missing partial index: {plan}"
    );
}

#[tokio::test]
async fn list_by_user_uses_partial_index_for_read() {
    let db = setup_db().await;
    let _ = create_test_user(&db, "u").await;
    let sql = r"
        SELECT e.id FROM entry e INDEXED BY idx_entry_read_sort
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1 AND e.read_at IS NOT NULL
        ORDER BY COALESCE(e.published_at, e.created_at) DESC
        LIMIT 51
    ";
    let plan = explain_plan_for(&db, sql, 1).await;
    assert!(
        plan.contains("idx_entry_read_sort"),
        "plan missing partial index: {plan}"
    );
}

#[tokio::test]
async fn list_by_user_no_predicate_uses_sort_ts_index() {
    // The "All Entries" SQL must carry the INDEXED BY hint.
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;
    for i in 1..=3 {
        crate::db_execute!(
            &db,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
            feed_id,
            format!("g{}", i),
            format!("2026-05-0{} 10:00:00", i)
        )
        .unwrap();
    }

    // Sanity: the public API returns the right rows under the hint.
    let rows = list_by_user(&db, &page(user_id, &EntryFilter::default(), 10, 0))
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);

    // Plan check on a hand-built copy of the query shape.
    let sql = r"
        SELECT e.id FROM entry e INDEXED BY idx_entry_sort_ts
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
        ORDER BY COALESCE(e.published_at, e.created_at) DESC
        LIMIT 51
    ";
    let plan = explain_plan_for(&db, sql, 1).await;
    assert!(
        plan.contains("idx_entry_sort_ts"),
        "plan missing sort_ts index: {plan}"
    );
}

#[tokio::test]
async fn continuation_page0_unfiltered_uses_sort_ts_index() {
    // Pins the page-0 hint in `list_by_user_with_continuation`: a dropped
    // hint gives identical results but a whole-corpus sort, so check the plan.
    let db = setup_db().await;
    let user_id = create_test_user(&db, "u").await;
    let cat_id = create_test_category(&db, user_id, "c").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;
    for i in 1..=3 {
        crate::db_execute!(
            &db,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
            feed_id,
            format!("g{}", i),
            format!("2026-05-0{} 10:00:00", i)
        )
        .unwrap();
    }

    // Hand-built copy of the default-filter, PublishedAt, no-continuation SQL.
    let sql = r"
        SELECT e.id
        FROM entry e INDEXED BY idx_entry_sort_ts
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE c.user_id = $1
        ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
        LIMIT $2
    ";
    let plan = explain_plan_for(&db, sql, 2).await;
    assert!(
        plan.contains("idx_entry_sort_ts"),
        "plan missing sort_ts index: {plan}"
    );
    assert!(
        !plan.contains("USE TEMP B-TREE FOR ORDER BY"),
        "plan must not temp-B-tree-sort: {plan}"
    );
}

/// Locks the plan for the snapshot-widened unread neighbours query (must not
/// fall back to an O(table) MULTI-INDEX OR).
#[tokio::test]
async fn find_neighbors_unread_read_after_uses_sort_ts_not_multi_index_or() {
    let db = setup_db().await;
    let _ = create_test_user(&db, "u").await;
    // Mirrors `find_neighbors`' next-side SQL; only the parameter count matters.
    let sql = r"
        SELECT e.id
        FROM entry e INDEXED BY idx_entry_sort_ts
        CROSS JOIN feed f ON e.feed_id = f.id
        CROSS JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
          AND (COALESCE(e.published_at, e.created_at) < $2
               OR (COALESCE(e.published_at, e.created_at) = $3 AND e.id < $4))
          AND (e.read_at IS NULL OR e.read_at >= $5)
        ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
        LIMIT 1
    ";
    let plan = explain_plan_for(&db, sql, 5).await;
    assert!(
        plan.contains("idx_entry_sort_ts"),
        "plan must pin idx_entry_sort_ts: {plan}"
    );
    assert!(
        !plan.contains("MULTI-INDEX OR"),
        "plan must not fan out into a MULTI-INDEX OR: {plan}"
    );
    assert!(
        !plan.contains("idx_entry_read_at"),
        "plan must not scan via the read_at index: {plan}"
    );
}

#[tokio::test]
async fn test_continuation_walk_is_gapless_unfiltered() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "walker").await;
    let category_id = create_test_category(&db, user_id, "C").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/walk.xml").await;
    // 5 entries, distinct published_at so order is deterministic.
    #[allow(
        clippy::cast_sign_loss,
        reason = "loop index from `0..5` is always non-negative"
    )]
    for i in 0..5 {
        upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("T{i}")),
                published_at: Some(
                    chrono::Utc
                        .with_ymd_and_hms(2024, 1, 1 + i as u32, 0, 0, 0)
                        .unwrap(),
                ),
                ..EntryUpsert::new(feed_id, &format!("g{i}"))
            },
        )
        .await
        .unwrap();
    }

    let filter = EntryFilter::default();
    let mut cursor: Option<ContinuationCursor> = None;
    let mut seen: Vec<i64> = Vec::new();
    loop {
        let params = ContinuationParams {
            oldest_first: false,
            limit: 3, // page size 2 + 1 sentinel
            continuation: cursor.clone(),
            ot: None,
            nt: None,
            sort_order: EntrySortOrder::PublishedAt,
        };
        let rows = list_by_user_with_continuation(&db, user_id, &filter, &params)
            .await
            .unwrap();
        let has_more = rows.len() > 2;
        let page = &rows[..rows.len().min(2)];
        if page.is_empty() {
            break;
        }
        for e in page {
            seen.push(e.entry.id);
        }
        if !has_more {
            break;
        }
        let last = page.last().unwrap();
        let ts = fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
            .await
            .unwrap()
            .unwrap();
        cursor = Some(ContinuationCursor::Composite {
            sort_ts: ts,
            id: last.entry.id,
        });
    }

    // All 5 seen exactly once, newest-first.
    assert_eq!(seen.len(), 5, "walk must visit every entry once: {seen:?}");
    let mut uniq = seen.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), 5, "no duplicates across pages: {seen:?}");
}

#[tokio::test]
async fn test_summarized_at_pages_by_summary_creation() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "summarizer").await;
    let category_id = create_test_category(&db, user_id, "C").await;
    let feed_id = create_test_feed(&db, category_id, "https://example.com/sum.xml").await;
    // Published oldest-to-newest g0..g4; summarized in the order g1, g3, g0,
    // g2 (g2 and g0 in the same second), g4 never.
    let mut ids = Vec::new();
    for i in 0..5u32 {
        let (e, _) = upsert_entry(
            &db,
            &EntryUpsert {
                title: Some(&format!("T{i}")),
                published_at: Some(
                    chrono::Utc
                        .with_ymd_and_hms(2024, 1, 1 + i, 0, 0, 0)
                        .unwrap(),
                ),
                ..EntryUpsert::new(feed_id, &format!("g{i}"))
            },
        )
        .await
        .unwrap();
        ids.push(e.id);
    }
    for (i, created_at) in [
        (1, "2024-02-01 00:00:00"),
        (3, "2024-02-02 00:00:00"),
        (0, "2024-02-03 00:00:00"),
        (2, "2024-02-03 00:00:00"),
    ] {
        crate::models::entry_summary::upsert_pending(&db, user_id, ids[i])
            .await
            .unwrap();
        exec_dynamic(
            &db,
            "UPDATE entry_summary SET created_at = $1 WHERE entry_id = $2".into(),
            vec![Bind::Text(created_at.into()), Bind::Int(ids[i])],
        )
        .await
        .unwrap();
    }

    let filter = EntryFilter {
        has_summary: Some(true),
        ..Default::default()
    };
    let mut cursor: Option<ContinuationCursor> = None;
    let mut seen: Vec<i64> = Vec::new();
    loop {
        let params = ContinuationParams {
            limit: 3, // page size 2 + 1 sentinel
            continuation: cursor.take(),
            sort_order: EntrySortOrder::SummarizedAt,
            ..Default::default()
        };
        let rows = list_by_user_with_continuation_ts(&db, user_id, &filter, &params)
            .await
            .unwrap();
        for (e, ts) in rows.iter().take(2) {
            let fetched = fetch_sort_ts(&db, e.entry.id, EntrySortOrder::SummarizedAt)
                .await
                .unwrap();
            assert_eq!(&fetched, ts, "cursor ts must match fetch_sort_ts");
            seen.push(e.entry.id);
        }
        let Some(next) = next_continuation(&rows, 2, |e| e.entry.id) else {
            break;
        };
        cursor = ContinuationCursor::parse(&next);
    }

    // Newest summary first; the same-second tie falls back to id DESC.
    assert_eq!(seen, vec![ids[2], ids[0], ids[3], ids[1]]);
}

#[tokio::test]
async fn test_prune_respects_threshold_star_and_optin() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "pruneuser").await;
    let cat_id = create_test_category(&db, user_id, "PruneCat").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/prune.xml").await;

    // Helper: insert a read entry aged `days_old` (and optionally starred).
    async fn mk(db: &Db, feed_id: i64, guid: &str, days: i64, starred: bool) {
        upsert_entry_id(
            db,
            &EntryUpsert {
                title: Some(guid),
                ..EntryUpsert::new(feed_id, guid)
            },
        )
        .await
        .unwrap();
        crate::db_execute!(
            db,
            "UPDATE entry SET read_at = datetime('now', $2) WHERE guid = $1 AND feed_id = $3",
            guid,
            format!("-{days} days"),
            feed_id
        )
        .unwrap();
        if starred {
            crate::db_execute!(
                db,
                "UPDATE entry SET starred_at = datetime('now') WHERE guid = $1 AND feed_id = $2",
                guid,
                feed_id
            )
            .unwrap();
        }
    }
    mk(&db, feed_id, "old", 40, false).await; // read, 40d, not starred -> victim once enabled
    mk(&db, feed_id, "oldstar", 40, true).await; // starred -> never deleted
    mk(&db, feed_id, "fresh", 1, false).await; // too recent -> kept
    upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("u"),
            ..EntryUpsert::new(feed_id, "unread")
        },
    )
    .await
    .unwrap(); // unread

    // Opt-in disabled (default 0): nothing pruned.
    assert_eq!(prune_read_retention_batch(&db, 500).await.unwrap(), 0);

    let user_id_check = crate::query_scalar!(
        &db,
        i64,
        "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = $1",
        feed_id
    )
    .unwrap();
    crate::models::user_settings::update_retention_read_days(&db, user_id_check, 30)
        .await
        .unwrap();

    // Only "old" is pruned; a tombstone is written for it.
    assert_eq!(prune_read_retention_batch(&db, 500).await.unwrap(), 1);
    assert!(
        find_by_guid_and_feed(&db, "old", feed_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        find_by_guid_and_feed(&db, "oldstar", feed_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        find_by_guid_and_feed(&db, "fresh", feed_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        find_by_guid_and_feed(&db, "unread", feed_id)
            .await
            .unwrap()
            .is_some()
    );

    // Tombstone present -> a refresh serving "old" again is skipped.
    let outcome = upsert_entry_id(
        &db,
        &EntryUpsert {
            title: Some("Old"),
            ..EntryUpsert::new(feed_id, "old")
        },
    )
    .await
    .unwrap();
    assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));

    // Idempotent: nothing left to prune.
    assert_eq!(prune_read_retention_batch(&db, 500).await.unwrap(), 0);
}

#[tokio::test]
async fn test_prune_batch_size_limits_rows() {
    let db = setup_db().await;
    let user_id = create_test_user(&db, "batchuser").await;
    let cat_id = create_test_category(&db, user_id, "BatchCat").await;
    let feed_id = create_test_feed(&db, cat_id, "https://example.com/batch.xml").await;
    let user_id_check = crate::query_scalar!(
        &db,
        i64,
        "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = $1",
        feed_id
    )
    .unwrap();
    crate::models::user_settings::update_retention_read_days(&db, user_id_check, 1)
        .await
        .unwrap();
    for i in 0..5 {
        let g = format!("g{i}");
        upsert_entry_id(
            &db,
            &EntryUpsert {
                title: Some(&g),
                ..EntryUpsert::new(feed_id, &g)
            },
        )
        .await
        .unwrap();
        crate::db_execute!(
            &db,
            "UPDATE entry SET read_at = datetime('now', '-10 days') WHERE guid = $1 AND feed_id = $2",
            g,
            feed_id
        )
        .unwrap();
    }
    assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 2);
    assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 2);
    assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 1);
    assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 0);

    // Every victim of every batch must be tombstoned, or it resurrects on refresh.
    for i in 0..5 {
        let g = format!("g{i}");
        assert!(
            find_by_guid_and_feed(&db, &g, feed_id)
                .await
                .unwrap()
                .is_none(),
            "{g} should have been deleted"
        );
        let outcome = upsert_entry_id(
            &db,
            &EntryUpsert {
                title: Some(&g),
                ..EntryUpsert::new(feed_id, &g)
            },
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, UpsertOutcome::SkippedTombstoned),
            "{g} should be tombstoned, got {outcome:?}"
        );
    }
}
