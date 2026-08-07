// static/js/csrf.js — echo the CSRF token back to the server on every
// state-changing request.
//
// The server hands the token to the page as a readable `csrf_token` cookie (see
// middleware::csrf). This module returns it in the two shapes the guard accepts,
// so no individual call site has to think about CSRF:
//
//   - as the `X-CSRF-Token` header on same-origin `fetch()` — covers the
//     form-swap layer, the bodyless keyboard-shortcut POSTs, and the JSON
//     endpoints (login, passkey, logout, unmasquerade);
//   - as a hidden `_csrf` field on native POST form submits — covers the forms
//     that post a body the browser serialises itself, including the multipart
//     OPML import.
//
// Loaded from base.html on every page, ahead of the feature modules, so the
// `fetch` patch is in place before anything calls it.

function csrfToken() {
  // The server writes __Host-csrf_token instead of csrf_token whenever the
  // deployment is Secure (see middleware::csrf::csrf_cookie_name on the Rust
  // side). Both names must be matched — a plain `csrf_token=` pattern does
  // NOT match `__Host-csrf_token=` (the character before `csrf_token` is
  // `-`, which fails the `(?:^|;\s*)` anchor), so on a Secure deployment
  // every JS-driven POST would silently send no token and get 403'd by the
  // synchronizer-token guard.
  //
  // __Host-csrf_token WINS when both are present, mirroring
  // middleware::auth::session_token_from_jar, which resolves the session the
  // guard derives the expected token from in exactly that order. The two
  // sides must agree: a browser can hold two cookie generations at once (an
  // unprefixed cookie minted before the deployment turned Secure, alongside
  // the prefixed one), and document.cookie orders by creation time, so
  // "whichever comes first" would hand back the *older* value while the
  // server validated against the newer session — 403 on every unsafe
  // request, for as long as the stale cookie lived. Preferring the prefix is
  // also a hardening win: a sibling subdomain can set `csrf_token` (cookie
  // tossing) but can never write a __Host- prefixed name.
  const read = (name) => {
    const m = document.cookie.match(
      new RegExp(`(?:^|;\\s*)${name}=([^;]*)`)
    );
    return m ? decodeURIComponent(m[1]) : "";
  };
  return read("__Host-csrf_token") || read("csrf_token");
}

const UNSAFE_METHOD = /^(POST|PUT|PATCH|DELETE)$/i;

// A relative URL, or an absolute one with our own origin, is same-origin. Only
// those get the token — it must never leak to a third-party host.
function isSameOrigin(url) {
  try {
    return new URL(url, location.href).origin === location.origin;
  } catch {
    return false;
  }
}

// Patch fetch so a same-origin state-changing request carries the token even
// though its call site never set it. A caller that already set the header wins.
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

// Inject `_csrf` into a native POST form before it submits. Capture phase, so it
// runs ahead of the form-swap handler that serialises the form into a body.
document.addEventListener(
  "submit",
  (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement)) return;
    if ((form.getAttribute("method") || "get").toUpperCase() !== "POST") return;
    if (form.querySelector('input[name="_csrf"]')) return;
    const token = csrfToken();
    if (!token) return;
    const input = document.createElement("input");
    input.type = "hidden";
    input.name = "_csrf";
    input.value = token;
    form.appendChild(input);
  },
  true
);
