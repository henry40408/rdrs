// Reports browser-only facts to the server, which answers with the hints to
// show when one disagrees with the account's settings. A report is sent only
// when it changed since the last one this browser delivered, so a mismatch the
// user ignored is not raised again.
//
// To report another fact: add it to `collect()` and handle it in
// `src/handlers/client_info.rs`.

function collect() {
  return { timezone: Intl.DateTimeFormat().resolvedOptions().timeZone };
}

// Per account: two users sharing a browser each get their own hint.
function storageKey() {
  return `rdrs.clientInfo:${document.documentElement.dataset.offlineKey || ''}`;
}

// Delivered means the server looked at it: a 2xx, or a 400 that retrying will
// not change (a zone name the server lacks). A 401/403, a redirect to the
// login page or a 5xx says nothing about the report, so it is sent again.
function wasDelivered(response) {
  if (response.redirected) return false;
  return response.ok || response.status === 400;
}

function showHints(reply) {
  const flash = document.querySelector('rdrs-flash');
  if (!flash) return;
  for (const hint of reply.hints || []) {
    flash.show('info', hint.message, {
      label: hint.action_label,
      href: hint.action_href,
    });
  }
}

async function report() {
  const body = JSON.stringify(collect());
  const key = storageKey();

  // Without storage nothing could remember the report, and it would repeat on
  // every page load.
  let last;
  try {
    last = localStorage.getItem(key);
  } catch {
    return;
  }
  if (last === body) return;

  let response;
  try {
    response = await fetch('/api/user/client-info', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body,
    });
  } catch {
    return; // offline: try again on a later page load
  }
  if (!wasDelivered(response)) return;

  try {
    localStorage.setItem(key, body);
  } catch {
    // Quota or private mode: the report repeats next time, which is harmless.
  }
  if (!response.ok) return;
  try {
    showHints(await response.json());
  } catch {
    // An unreadable reply loses this one hint; the report counts as delivered.
  }
}

report();
