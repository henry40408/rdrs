import Database from "better-sqlite3";

export class SeedHelper {
  constructor(dbPath) {
    this.dbPath = dbPath;
  }

  insertEntries(entries) {
    const db = new Database(this.dbPath);
    const ids = [];
    try {
      const stmt = db.prepare(
        `INSERT OR IGNORE INTO entry (feed_id, guid, title, link, content, summary, published_at)
         VALUES (?, ?, ?, ?, ?, ?, datetime('now', ?))`
      );
      const insertAll = db.transaction(() => {
        for (const entry of entries) {
          const result = stmt.run(
            entry.feedId,
            entry.guid,
            entry.title,
            entry.link,
            entry.content,
            entry.summary ?? null,
            entry.publishedOffset ?? "0 seconds"
          );
          ids.push(Number(result.lastInsertRowid));
        }
      });
      insertAll();
    } finally {
      db.close();
    }
    return ids;
  }

  getUserId(username) {
    const db = new Database(this.dbPath);
    try {
      const row = db.prepare(`SELECT id FROM user WHERE username = ?`).get(username);
      if (!row) throw new Error(`User '${username}' not found`);
      return row.id;
    } finally {
      db.close();
    }
  }

  createCategory(userId, name) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`INSERT OR IGNORE INTO category (user_id, name) VALUES (?, ?)`).run(userId, name);
      const row = db.prepare(`SELECT id FROM category WHERE user_id = ? AND name = ?`).get(userId, name);
      return row.id;
    } finally {
      db.close();
    }
  }

  createFeed(categoryId, url, title) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`INSERT OR IGNORE INTO feed (category_id, url, title) VALUES (?, ?, ?)`).run(
        categoryId,
        url,
        title ?? url
      );
      const row = db.prepare(`SELECT id FROM feed WHERE url = ?`).get(url);
      return row.id;
    } finally {
      db.close();
    }
  }

  insertIcon(feedId, data, contentType, sourceUrl) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(
        `INSERT OR REPLACE INTO image (entity_type, entity_id, data, content_type, source_url)
         VALUES ('feed', ?, ?, ?, ?)`
      ).run(feedId, data, contentType, sourceUrl ?? null);
    } finally {
      db.close();
    }
  }

  markRead(entryId, relativeTime = "0 seconds") {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`UPDATE entry SET read_at = datetime('now', ?) WHERE id = ?`).run(relativeTime, entryId);
    } finally {
      db.close();
    }
  }

  markStarred(entryId, relativeTime = "0 seconds") {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`UPDATE entry SET starred_at = datetime('now', ?) WHERE id = ?`).run(relativeTime, entryId);
    } finally {
      db.close();
    }
  }

  insertSummary(entryId, userId, text = "summary.") {
    const db = new Database(this.dbPath);
    try {
      db.prepare(
        `INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, summary_text)
         VALUES (?, ?, 'completed', ?)`
      ).run(userId, entryId, text);
    } finally {
      db.close();
    }
  }

  seedTestEntries(feedId, count) {
    const entries = [];
    for (let i = 1; i <= count; i++) {
      entries.push({
        feedId,
        guid: `test-guid-${feedId}-${i}`,
        title: `Test Entry ${i}`,
        link: `https://example.com/entry/${i}`,
        content: `<p>Content for test entry ${i}</p>`,
        summary: `Summary for entry ${i}`,
        publishedOffset: `-${i} hours`,
      });
    }
    return this.insertEntries(entries);
  }
}
