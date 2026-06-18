// static/js/statistics.js — Statistics page enhancements.
//
// Keeps the Daily Read chart's hover/focus tooltip inside the chart box.
// Bars are bucketed to a fixed count, so a tooltip centred over a bar near
// the chart edge would otherwise be clipped by the chart's `overflow-x: clip`
// (the no-JS safety net that stops the page scrolling). Here we measure the
// tooltip on interaction and shift it horizontally just enough to fit.
//
// Progressive enhancement: without this script the tooltips still appear —
// the outermost ones simply clip at the edge, and the full value remains in
// each bar's `aria-label`.

/**
 * Shift `col`'s tooltip horizontally so it stays within the chart, starting
 * from its CSS baseline (`left: 50%` + `translateX(-50%)`).
 */
function placeTooltip(chart, col) {
    const tip = col.querySelector('.stats-bar-tip');
    if (!tip) return;
    const PAD = 4;
    // Reset to the centred baseline before measuring.
    tip.style.transform = 'translateX(-50%)';
    const chartRect = chart.getBoundingClientRect();
    const tipRect = tip.getBoundingClientRect();
    let shift = 0;
    if (tipRect.left < chartRect.left + PAD) {
        shift = chartRect.left + PAD - tipRect.left;
    } else if (tipRect.right > chartRect.right - PAD) {
        shift = chartRect.right - PAD - tipRect.right;
    }
    if (shift !== 0) {
        tip.style.transform = `translateX(calc(-50% + ${Math.round(shift)}px))`;
    }
}

function installChartTooltips() {
    const chart = document.querySelector('.stats-chart');
    if (!chart) return;
    chart.querySelectorAll('.stats-bar-col').forEach((col) => {
        // Recompute on each interaction so viewport resizes are handled for free.
        col.addEventListener('pointerenter', () => placeTooltip(chart, col));
        col.addEventListener('focus', () => placeTooltip(chart, col));
    });
}

installChartTooltips();
