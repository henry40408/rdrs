// Keeps Daily Read tooltips inside the chart, where `overflow-x: clip` would
// otherwise cut edge ones. Without JS they just clip; `aria-label` has the value.

/** Shift `col`'s tooltip horizontally so it stays within the chart. */
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
