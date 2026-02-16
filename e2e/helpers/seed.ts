import Database from "better-sqlite3";

export interface SeedEntry {
  feedId: number;
  guid: string;
  title: string;
  link: string;
  content: string;
  summary?: string;
  /** Relative time offset, e.g. "-1 hours". Defaults to "0 seconds". */
  publishedOffset?: string;
}

/** Direct SQLite helper for seeding entry data. */
export class SeedHelper {
  constructor(private dbPath: string) {}

  /** Insert entries directly into the SQLite database. Returns inserted entry IDs. */
  insertEntries(entries: SeedEntry[]): number[] {
    const db = new Database(this.dbPath);
    const ids: number[] = [];

    try {
      const stmt = db.prepare(
        `INSERT INTO entry (feed_id, guid, title, link, content, summary, published_at)
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

  /** Get a user's id by username. */
  getUserId(username: string): number {
    const db = new Database(this.dbPath);
    try {
      const row = db
        .prepare(`SELECT id FROM user WHERE username = ?`)
        .get(username) as { id: number } | undefined;
      if (!row) throw new Error(`User '${username}' not found`);
      return row.id;
    } finally {
      db.close();
    }
  }

  /** Insert a category directly into SQLite. Returns the category id. */
  createCategory(userId: number, name: string): number {
    const db = new Database(this.dbPath);
    try {
      const result = db
        .prepare(`INSERT INTO category (user_id, name) VALUES (?, ?)`)
        .run(userId, name);
      return Number(result.lastInsertRowid);
    } finally {
      db.close();
    }
  }

  /** Insert a feed directly into SQLite (bypasses URL fetch). Returns the feed id. */
  createFeed(categoryId: number, url: string, title?: string): number {
    const db = new Database(this.dbPath);
    try {
      const result = db
        .prepare(
          `INSERT INTO feed (category_id, url, title) VALUES (?, ?, ?)`
        )
        .run(categoryId, url, title ?? url);
      return Number(result.lastInsertRowid);
    } finally {
      db.close();
    }
  }

  /** Generate and insert N test entries for a feed. Returns entry IDs. */
  seedTestEntries(feedId: number, count: number): number[] {
    const entries: SeedEntry[] = [];
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
