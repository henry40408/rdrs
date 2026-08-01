// The bootstrap admin for each worker's server, keyed by base URL.
//
// rdrs has no public sign-up: `/api/setup` creates the very first account and
// then closes for good, and every later account is created by an admin who
// hands out a one-time link. Scenarios still want a throwaway user each, so the
// first call per server claims the setup endpoint and everything after it goes
// through the real admin + invite flow.
const bootstrapAdmins = new Map();

export class ApiHelper {
  constructor(baseUrl) {
    this.baseUrl = baseUrl;
  }

  /// Create an account with a password, whatever it takes.
  async register(username, password) {
    const admin = await this.#ensureAdmin();
    if (admin.username === username) return;

    const session = await this.login(admin.username, admin.password);
    const invitePath = await this.#createAccount(session, username);
    await this.#redeemInvite(invitePath, password);
  }

  /// Claim the one-time setup endpoint for `username`.
  ///
  /// The account it creates is the instance's administrator, which is what the
  /// README screenshots depict — a single-user install, sidebar and all. Going
  /// through `register` instead would create an ordinary member account and
  /// quietly drop the admin entries from every captured sidebar.
  async setupFirstAccount(username, password) {
    const res = await fetch(`${this.baseUrl}/api/setup`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Setup failed (${res.status}): ${body}`);
    }
    bootstrapAdmins.set(this.baseUrl, { username, password });
  }

  /// Create an account and hand back its one-time link, unredeemed.
  ///
  /// The half of `register` that stops before choosing a password, for
  /// scenarios that drive the invite page in the browser.
  async inviteAccount(username) {
    const admin = await this.#ensureAdmin();
    const session = await this.login(admin.username, admin.password);
    return this.#createAccount(session, username);
  }

  async #ensureAdmin() {
    const existing = bootstrapAdmins.get(this.baseUrl);
    if (existing) return existing;

    const admin = {
      username: `e2e-bootstrap-${Math.random().toString(36).slice(2, 10)}`,
      password: "vulture-mango-77-quilt",
    };
    const res = await fetch(`${this.baseUrl}/api/setup`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(admin),
    });
    if (!res.ok) {
      const body = await res.text();
      throw new Error(`Setup failed (${res.status}): ${body}`);
    }
    bootstrapAdmins.set(this.baseUrl, admin);
    return admin;
  }

  async #createAccount(session, username) {
    const res = await fetch(`${this.baseUrl}/admin/users`, {
      method: "POST",
      redirect: "manual",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        Cookie: session.cookie,
        "X-CSRF-Token": session.csrf,
      },
      body: new URLSearchParams({ username, role: "user" }).toString(),
    });
    if (res.status !== 303) {
      const body = await res.text();
      throw new Error(`Create account failed (${res.status}): ${body}`);
    }

    // The link is shown once, in the flash cookie, and stored only as an HMAC
    // — reading it here is exactly what an admin does on the page.
    const flash = (res.headers.getSetCookie?.() ?? []).find((c) => c.startsWith("flash="));
    const decoded = decodeURIComponent(flash ?? "");
    const match = decoded.match(/\/invite\/[A-Za-z0-9_-]+/);
    if (!match) throw new Error(`No invite link in flash: ${decoded}`);
    return match[0];
  }

  async #redeemInvite(invitePath, password) {
    // Load the page first: the anonymous-session middleware mints the session
    // and readable CSRF cookie on that GET, and the synchronizer-token guard
    // wants the token echoed back on the POST. In a browser csrf.js does this;
    // here it is done by hand.
    const page = await fetch(`${this.baseUrl}${invitePath}`);
    const pageCookies = page.headers.getSetCookie?.() ?? [];
    const cookie = pageCookies.map((c) => c.split(";")[0]).join("; ");
    const csrf =
      pageCookies
        .find((c) => c.startsWith("csrf_token="))
        ?.split(";")[0]
        .split("=")[1] ?? "";

    const res = await fetch(`${this.baseUrl}${invitePath}`, {
      method: "POST",
      redirect: "manual",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        Cookie: cookie,
        "X-CSRF-Token": decodeURIComponent(csrf),
      },
      body: new URLSearchParams({ password, confirm_password: password }).toString(),
    });
    if (res.status !== 303) {
      const body = await res.text();
      throw new Error(`Invite redemption failed (${res.status}): ${body}`);
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
    // The synchronizer-token guard wants this echoed back as a header on every
    // state-changing request, which is what csrf.js does in the browser.
    const csrf =
      setCookie
        .find((c) => c.startsWith("csrf_token="))
        ?.split(";")[0]
        .split("=")[1] ?? "";
    return { cookie, csrf };
  }
}
