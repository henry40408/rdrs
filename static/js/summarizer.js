// Summarizer page: drive queued cards one at a time. Progressive enhancement —
// without JS the server-rendered "Queued" cards simply stay put.
const results = document.querySelector('[data-summarizer-results]');
if (results) {
  // A single serial runner keeps exactly one request in flight at a time, so
  // there is never more than one AbortController to track — Retry can re-queue
  // a card without clobbering the in-flight card's controller.
  let running = false; // a walk is in progress
  let currentController = null; // AbortController for the single in-flight fetch

  const CANCEL_ACTIONS =
    '<button type="button" class="rp-action" data-sz-cancel aria-label="Cancel summarization"><span class="action-label">Cancel</span></button>';
  const RECOVER_ACTIONS =
    '<button type="button" class="rp-action" data-sz-retry aria-label="Retry summarization"><span class="action-label">Retry</span></button>' +
    '<button type="button" class="rp-action" data-sz-dismiss aria-label="Dismiss"><span class="action-label">Dismiss</span></button>';

  const setActions = (card, html) => {
    const actions = card.querySelector('[data-sz-actions]');
    if (actions) actions.innerHTML = html;
  };

  const setStatus = (card, html) => {
    let status = card.querySelector('[data-sz-status]');
    if (!status) {
      status = document.createElement('p');
      status.className = 'status';
      status.setAttribute('data-sz-status', '');
      card.appendChild(status);
    }
    status.innerHTML = html;
  };

  const setSummarizing = (card) => {
    card.dataset.state = 'summarizing';
    card.classList.remove('pending');
    setActions(card, CANCEL_ACTIONS);
    setStatus(card, '<span class="sz-spinner" aria-hidden="true"></span>Summarizing…');
  };

  // Leave a card in a recoverable error state (Retry + Dismiss) with a message.
  const setRecoverable = (card, message) => {
    card.dataset.state = 'error';
    card.classList.remove('pending');
    setActions(card, RECOVER_ACTIONS);
    setStatus(card, message);
  };

  // Summarize one card. Returns true if the walk should continue, false if the
  // request was cancelled (which halts the remaining queue).
  const summarizeCard = async (card) => {
    const url = card.dataset.summarizerUrl;
    const index = card.dataset.summarizerIndex;
    setSummarizing(card);
    const controller = new AbortController();
    currentController = controller;
    try {
      const res = await fetch('/summarizer/item', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ url, index }),
        signal: controller.signal,
      });
      const html = await res.text();
      const tmp = document.createElement('div');
      tmp.innerHTML = html.trim();
      const fresh = tmp.firstElementChild;
      if (fresh) {
        // Server fragment is already a completed/error card — swap it in.
        card.replaceWith(fresh);
      } else {
        // Malformed/empty fragment: don't leave the card stuck on the spinner.
        setRecoverable(card, 'Summarization failed — Retry to try again.');
      }
      return true; // resolved — continue the walk
    } catch (e) {
      if (e.name === 'AbortError') {
        setRecoverable(card, 'Cancelled — Retry to run this one again.');
        return false; // halt the remaining queue
      }
      setRecoverable(card, 'Network error — Retry to try again.');
      return true; // recoverable, but keep going with the rest
    } finally {
      // Only clear if this invocation still owns the slot (it always should,
      // since the runner is serial — guard is defensive).
      if (currentController === controller) currentController = null;
    }
  };

  // Serial runner: process queued cards top-to-bottom. Re-entrant-safe — a
  // second call while a walk is active is a no-op; the caller's newly-queued
  // card is picked up by the running loop (it re-queries each iteration).
  const runQueue = async () => {
    if (running) return;
    running = true;
    try {
      for (;;) {
        const next = results.querySelector('[data-summarizer-card][data-state="queued"]');
        if (!next) break;
        const keepGoing = await summarizeCard(next);
        if (!keepGoing) break; // cancelled — stop the remaining queue
      }
    } finally {
      running = false;
    }
  };

  results.addEventListener('click', (e) => {
    if (e.target.closest('[data-sz-cancel]')) {
      currentController?.abort();
      return;
    }
    const retry = e.target.closest('[data-sz-retry]');
    if (retry) {
      const card = retry.closest('[data-summarizer-card]');
      if (card) {
        card.dataset.state = 'queued';
        card.classList.add('pending');
        setActions(card, '');
        setStatus(card, 'Queued');
        runQueue(); // no-op if a walk is active; the loop picks this card up
      }
      return;
    }
    const dismiss = e.target.closest('[data-sz-dismiss]');
    if (dismiss) {
      dismiss.closest('[data-summarizer-card]')?.remove();
      return;
    }
    const copy = e.target.closest('[data-sz-copy]');
    if (copy) {
      const body = copy.closest('[data-summarizer-card]')?.querySelector('[data-sz-body]');
      if (body) navigator.clipboard?.writeText(body.textContent.trim());
    }
  });

  runQueue();
}
