// Progressive enhancement: without JS the server-rendered "Queued" cards stay.
const results = document.querySelector('[data-summarizer-results]');
if (results) {
  // One request in flight at a time, so there is never more than one
  // AbortController to track and Retry can re-queue a card without clobbering it.
  let running = false;
  let currentController = null;

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

  // True to continue the walk, false when cancellation halts the queue.
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
        // The fragment is already a completed/error card.
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
      // Defensive: the serial runner means this should always own the slot.
      if (currentController === controller) currentController = null;
    }
  };

  // Re-entrant-safe: a second call during a walk is a no-op, and the newly
  // queued card is picked up by the running loop, which re-queries each pass.
  const runQueue = async () => {
    if (running) return;
    running = true;
    try {
      for (;;) {
        const next = results.querySelector('[data-summarizer-card][data-state="queued"]');
        if (!next) break;
        const keepGoing = await summarizeCard(next);
        if (!keepGoing) {
          // Stop auto-processing, but leave every queued card recoverable
          // rather than stranded without controls.
          results
            .querySelectorAll('[data-summarizer-card][data-state="queued"]')
            .forEach((c) => setRecoverable(c, 'Stopped — Retry to run this one.'));
          break;
        }
      }
    } finally {
      running = false;
    }
  };

  results.addEventListener('click', async (e) => {
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
      const card = copy.closest('[data-summarizer-card]');
      const body = card?.querySelector('[data-sz-body]');
      if (body) {
        // Mirrors the entry-summary copy, so the text keeps its source context.
        const title = (card.querySelector('[data-sz-title]')?.textContent || '').trim();
        const url = (card.dataset.summarizerUrl || '').trim();
        const summary = body.textContent.trim();
        const parts = [];
        if (title) parts.push(title);
        if (url) parts.push(url);
        parts.push(summary);
        try {
          await navigator.clipboard.writeText(parts.join('\n\n'));
          // Only the label span, so any icon span survives.
          const label = copy.querySelector('.action-label') || copy;
          const original = label.textContent;
          label.textContent = 'Copied!';
          setTimeout(() => { label.textContent = original; }, 2000);
        } catch {
          window.flash?.error('Failed to copy to clipboard');
        }
      }
    }
  });

  runQueue();
}
