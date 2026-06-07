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
      const idLookup = db.prepare(
        `SELECT id FROM entry WHERE feed_id = ? AND guid = ?`
      );
      const insertAll = db.transaction(() => {
        for (const entry of entries) {
          stmt.run(
            entry.feedId,
            entry.guid,
            entry.title,
            entry.link,
            entry.content,
            entry.summary ?? null,
            entry.publishedOffset ?? "0 seconds"
          );
          const row = idLookup.get(entry.feedId, entry.guid);
          ids.push(row.id);
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

  configureKagi(userId, sessionToken = "e2e-test-token") {
    // Seeds a fake Kagi config so the reading-pane Summarize button is rendered.
    // The token is bogus — actual Kagi calls will fail at request time, which
    // is fine for tests that only assert UI state up through the in-flight
    // placeholder.
    const db = new Database(this.dbPath);
    try {
      const payload = JSON.stringify({ kagi: { session_token: sessionToken } });
      const existing = db
        .prepare(`SELECT id FROM user_settings WHERE user_id = ?`)
        .get(userId);
      if (existing) {
        db.prepare(
          `UPDATE user_settings SET save_services = ? WHERE user_id = ?`
        ).run(payload, userId);
      } else {
        db.prepare(
          `INSERT INTO user_settings (user_id, save_services) VALUES (?, ?)`
        ).run(userId, payload);
      }
    } finally {
      db.close();
    }
  }

  makeAdmin(userId) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(`UPDATE user SET role = 'admin' WHERE id = ?`).run(userId);
    } finally {
      db.close();
    }
  }

  findEntryIdByTitle(userId, title) {
    const db = new Database(this.dbPath);
    try {
      const row = db
        .prepare(
          `SELECT e.id FROM entry e
           JOIN feed f ON e.feed_id = f.id
           JOIN category c ON f.category_id = c.id
           WHERE c.user_id = ? AND e.title = ?`
        )
        .get(userId, title);
      if (!row) throw new Error(`Entry '${title}' not found`);
      return row.id;
    } finally {
      db.close();
    }
  }

  findFeedIdByTitle(userId, feedTitle) {
    const db = new Database(this.dbPath);
    try {
      const row = db
        .prepare(
          `SELECT f.id FROM feed f
           JOIN category c ON f.category_id = c.id
           WHERE c.user_id = ? AND f.title = ?`
        )
        .get(userId, feedTitle);
      if (!row) throw new Error(`Feed '${feedTitle}' not found`);
      return row.id;
    } finally {
      db.close();
    }
  }

  firstFeedId(userId) {
    const db = new Database(this.dbPath);
    try {
      const row = db
        .prepare(
          `SELECT f.id FROM feed f
           JOIN category c ON f.category_id = c.id
           WHERE c.user_id = ? LIMIT 1`
        )
        .get(userId);
      if (!row) throw new Error("No feed found for user");
      return row.id;
    } finally {
      db.close();
    }
  }

  findCategoryIdByName(userId, name) {
    const db = new Database(this.dbPath);
    try {
      const row = db
        .prepare(`SELECT id FROM category WHERE user_id = ? AND name = ?`)
        .get(userId, name);
      if (!row) throw new Error(`Category '${name}' not found`);
      return row.id;
    } finally {
      db.close();
    }
  }

  markCategoryRead(userId, categoryName) {
    const db = new Database(this.dbPath);
    try {
      db.prepare(
        `UPDATE entry SET read_at = datetime('now')
         WHERE feed_id IN (
           SELECT f.id FROM feed f
           JOIN category c ON f.category_id = c.id
           WHERE c.user_id = ? AND c.name = ?
         )`
      ).run(userId, categoryName);
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
