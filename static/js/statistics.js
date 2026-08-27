// static/js/statistics.js — keep the Daily Read chart's tooltip inside the chart
// box. A tooltip centred over a bar near the edge is otherwise clipped by
// `overflow-x: clip`, the no-JS safety net that stops the page scrolling.
//
// Progressive enhancement: without this the tooltips still appear, the outermost
// ones simply clip, and the full value stays in each bar's `aria-label`.

/**
 * Shift `col`'s tooltip horizontally so it stays within the chart, starting from
 * its CSS baseline (`left: 50%` + `translateX(-50%)`).
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
            col.addEventListener('pointerenter', () => placeTooltip(chart, col));
        col.addEventListener('focus', () => placeTooltip(chart, col));
    });
}

installChartTooltips();
