// static/js/csrf.js — echo the CSRF token back to the server on every
// state-changing request: as the `X-CSRF-Token` header on same-origin `fetch()`,
// and as a hidden `_csrf` field on native POST form submits. Loaded from
// base.html ahead of the feature modules, so the `fetch` patch is in place
// before anything calls it.

function csrfToken() {
  // Both names must be matched: the server writes __Host-csrf_token on a Secure
  // deployment (middleware::csrf::csrf_cookie_name), and a plain `csrf_token=`
  // pattern does not match it — the `-` before the name fails the `(?:^|;\s*)`
  // anchor, so every JS-driven POST would send no token and get 403'd.
  //
  // __Host- wins when both are present, mirroring the resolution order in
  // middleware::auth::session_token_from_jar. A browser can hold both cookie
  // generations at once and document.cookie orders by creation time, so
  // "whichever comes first" would validate an older value against a newer
  // session. Preferring the prefix also blocks cookie tossing: a sibling
  // subdomain can set `csrf_token`, never a __Host- prefixed name.
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

// Capture phase, so this runs ahead of the form-swap handler that serialises the
// form into a body.
//
// The server renders the field (the `csrf_field` macro), so the usual job is to
// overwrite: `slide_session_cookie` rotates the session token on the way out of
// a response, leaving the markup's snapshot staler than the cookie `csrf_guard`
// checks against. Still created when missing, for any form that predates the
// server-side rendering.
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
