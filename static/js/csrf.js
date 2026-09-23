// Echoes the CSRF token as `X-CSRF-Token` on same-origin fetch and `_csrf` on
// native POST forms. Loaded first so the fetch patch precedes any request.

function csrfToken() {
  // Match both names (plain `csrf_token=` won't match `__Host-csrf_token`).
  // __Host- wins, mirroring session_token_from_jar: it avoids pairing an older
  // cookie with a newer session and blocks cookie tossing from subdomains.
  const read = (name) => {
    const m = document.cookie.match(
      new RegExp(`(?:^|;\\s*)${name}=([^;]*)`)
    );
    return m ? decodeURIComponent(m[1]) : "";
  };
  return read("__Host-csrf_token") || read("csrf_token");
}

const UNSAFE_METHOD = /^(POST|PUT|PATCH|DELETE)$/i;

// Only same-origin requests get the token — it must never leak to a third party.
function isSameOrigin(url) {
  try {
    return new URL(url, location.href).origin === location.origin;
  } catch {
    return false;
  }
}

// A call site that already set the header wins.
const nativeFetch = window.fetch.bind(window);
window.fetch = function (input, init) {
  const isRequest = typeof Request !== "undefined" && input instanceof Request;
  const method = (init && init.method) || (isRequest ? input.method : "GET");
  const url = isRequest ? input.url : input;
  if (UNSAFE_METHOD.test(method) && isSameOrigin(url)) {
    const token = csrfToken();
    if (token) {
      const headers = new Headers(
        (init && init.headers) || (isRequest ? input.headers : undefined)
      );
      if (!headers.has("X-CSRF-Token")) headers.set("X-CSRF-Token", token);
      init = { ...(init || {}), headers };
    }
  }
  return nativeFetch(input, init);
};

// Capture phase, ahead of the form-swap handler that serialises the body.
// Overwrites the server-rendered field: session rotation can leave it stale.
document.addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
    if ((form.getAttribute("method") || "get").toUpperCase() !== "POST") return;
    const token = csrfToken();
    if (!token) return;
    const existing = form.querySelector('input[name="_csrf"]');
    if (existing) {
      existing.value = token;
      return;
    }
    const input = document.createElement("input");
    input.type = "hidden";
    input.name = "_csrf";
    input.value = token;
    form.appendChild(input);
  },
  true
);
