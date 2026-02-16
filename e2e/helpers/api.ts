/** REST API helper for E2E tests. */
export class ApiHelper {
  constructor(private baseUrl: string) {}

  /** Register a new user. Returns the session cookie. */
  async register(
    username: string,
    password: string
  ): Promise<{ cookie: string }> {
    const res = await fetch(`${this.baseUrl}/api/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Register failed (${res.status}): ${body}`);
    }
    return { cookie: "" };
  }

  /** Login and return the session cookie string. */
  async login(
    username: string,
    password: string
  ): Promise<{ cookie: string }> {
    const res = await fetch(`${this.baseUrl}/api/session`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
      redirect: "manual",
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Login failed (${res.status}): ${body}`);
    }
    const setCookie = res.headers.getSetCookie?.() ?? [];
    const cookie = setCookie
      .map((c) => c.split(";")[0])
      .join("; ");
    return { cookie };
  }

  /** Create a category. Returns its id. */
  async createCategory(cookie: string, name: string): Promise<number> {
    const res = await fetch(`${this.baseUrl}/api/categories`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Cookie: cookie,
      },
      body: JSON.stringify({ name }),
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Create category failed (${res.status}): ${body}`);
    }
    const data = (await res.json()) as { id: number };
    return data.id;
  }

  /** Create a feed. Returns its id. */
  async createFeed(
    cookie: string,
    url: string,
    categoryId: number
  ): Promise<number> {
    const res = await fetch(`${this.baseUrl}/api/feeds`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Cookie: cookie,
      },
      body: JSON.stringify({ url, category_id: categoryId }),
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Create feed failed (${res.status}): ${body}`);
    }
    const data = (await res.json()) as { id: number };
    return data.id;
  }
}
