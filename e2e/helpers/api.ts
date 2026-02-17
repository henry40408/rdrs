/** REST API helper for E2E tests. */
export class ApiHelper {
  constructor(private baseUrl: string) {}

  /** Register a new user. Idempotent: ignores 409 (already exists). */
  async register(
    username: string,
    password: string
  ): Promise<{ cookie: string }> {
    const res = await fetch(`${this.baseUrl}/api/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok && res.status !== 409) {
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

}
