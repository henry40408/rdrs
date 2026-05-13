export class ApiHelper {
  constructor(baseUrl) {
    this.baseUrl = baseUrl;
  }

  async register(username, password) {
    const res = await fetch(`${this.baseUrl}/api/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok && res.status !== 409) {
      const body = await res.text();
      throw new Error(`Register failed (${res.status}): ${body}`);
    }
  }

  async login(username, password) {
    const res = await fetch(`${this.baseUrl}/api/session`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Login failed (${res.status}): ${body}`);
    }
    const setCookie = res.headers.getSetCookie?.() ?? [];
    const cookie = setCookie.map((c) => c.split(";")[0]).join("; ");
    return { cookie };
  }
}
