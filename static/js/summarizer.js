// Summarizer page: drive queued cards one at a time. Progressive enhancement —
// without JS the server-rendered "Queued" cards simply stay put.
const results = document.querySelector('[data-summarizer-results]');
if (results) {
  let current = null; // AbortController for the in-flight card

  const setSummarizing = (card) => {
    card.dataset.state = 'summarizing';
    card.classList.remove('pending');
    const actions = card.querySelector('[data-sz-actions]');
    if (actions) {
      actions.innerHTML = '<button type="button" class="rp-action" data-sz-cancel aria-label="Cancel summarization"><span class="action-label">Cancel</span></button>';
    }
    let status = card.querySelector('[data-sz-status]');
    if (!status) {
      status = document.createElement('p');
      status.className = 'status';
      status.setAttribute('data-sz-status', '');
      card.appendChild(status);
    }
    status.innerHTML = '<span class="sz-spinner" aria-hidden="true"></span>Summarizing…';
  };

  const summarizeCard = async (card) => {
    const url = card.dataset.summarizerUrl;
    const index = card.dataset.summarizerIndex;
    setSummarizing(card);
    const controller = new AbortController();
    current = controller;
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
      if (fresh) card.replaceWith(fresh);
      return true; // resolved (completed or error) — continue the walk
    } catch (e) {
      if (e.name === 'AbortError') return false; // cancelled — stop the walk
      // Network error: render an inline error and continue.
      const status = card.querySelector('[data-sz-status]');
      if (status) status.textContent = 'Network error — Retry from the button.';
      card.dataset.state = 'error';
      return true;
    } finally {
      current = null;
    }
  };

  const run = async () => {
    // Re-query each iteration: replaceWith() swaps nodes.
    for (;;) {
      const next = results.querySelector('[data-summarizer-card][data-state="queued"]');
      if (!next) break;
      const cont = await summarizeCard(next);
      if (!cont) break;
    }
  };

  results.addEventListener('click', (e) => {
    const cancel = e.target.closest('[data-sz-cancel]');
    if (cancel) {
      current?.abort();
      const card = cancel.closest('[data-summarizer-card]');
      // Mark remaining queued cards as cancelled visually (leave them dimmed).
      if (card) card.dataset.state = 'error';
      return;
    }
    const retry = e.target.closest('[data-sz-retry]');
    if (retry) {
      const card = retry.closest('[data-summarizer-card]');
      if (card) { card.dataset.state = 'queued'; summarizeCard(card); }
      return;
    }
    const dismiss = e.target.closest('[data-sz-dismiss]');
    if (dismiss) { dismiss.closest('[data-summarizer-card]')?.remove(); return; }
    const copy = e.target.closest('[data-sz-copy]');
    if (copy) {
      const body = copy.closest('[data-summarizer-card]')?.querySelector('[data-sz-body]');
      if (body) navigator.clipboard?.writeText(body.textContent.trim());
    }
  });

  run();
}
