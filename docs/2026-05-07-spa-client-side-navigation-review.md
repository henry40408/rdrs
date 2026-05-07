# SPA Client-Side Navigation Review

日期：2026-05-07

## 背景

目前 UI 理論上在初次載入後，站內頁面切換應該都走 client-side navigation。這份 review 檢查重點是：哪些路徑、互動或 fallback 還可能造成 full document reload。

## Findings

### 1. `/entries/{id}` 仍可能透過 history / popstate 觸發整頁載入

嚴重度：High

`static/js/components/rdrs-entry-list.js:515` 會把文章閱讀狀態推進 history，URL 形式是 `/entries/{id}?...`。但 `static/js/router.js:17` 的 `ROUTES` 沒有包含 `/entries/\d+`，因此當瀏覽器 Back / Forward 回到這種 URL 時，router 會在 `static/js/router.js:41` 走 fallback `location.href = path`，造成 full reload。

可重現路徑：

1. 進入 `/entries` 或 `/`。
2. 點開一篇文章，URL 變成 `/entries/{id}?origin=...`。
3. 切換到另一個 routed page，例如 `/feeds`。
4. 使用瀏覽器 Back / Forward 回到 `/entries/{id}`。

預期：仍在目前 document 內 client-side 切回文章狀態。

實際風險：router 不認得 `/entries/{id}`，因此可能觸發 document navigation。現有 e2e 只測 page-level Back / Forward，沒有覆蓋 entry-detail history case。

相關位置：

- `static/js/components/rdrs-entry-list.js:515`
- `static/js/components/rdrs-entry-list.js:517`
- `static/js/components/rdrs-entry-list.js:519`
- `static/js/router.js:17`
- `static/js/router.js:41`
- `e2e/tests/spa-router.spec.ts:86`

### 2. Statistics 自訂日期 Apply 還是一般 GET form submit

嚴重度：Medium

`static/js/pages/statistics.js:26` render 出 `<form class="stats-period" method="get" action="/statistics">`，但沒有攔截 submit。7d / 30d / 90d / All 這些 `<a href="/statistics?...">` 會被 SPA router 攔截；自訂日期按下 Apply 則會走瀏覽器原生 form navigation，造成 full reload。

可重現路徑：

1. 進入 `/statistics`。
2. 選擇 from / to 日期。
3. 按 Apply。

預期：client-side 更新 URL 與資料。

實際風險：整頁 GET `/statistics?period=custom&from=...&to=...`。

相關位置：

- `static/js/pages/statistics.js:26`
- `static/js/pages/statistics.js:36`

### 3. 多個 post-action flow 仍透過 `flash.redirect()` hard reload

嚴重度：Depends on intended scope

`static/js/components/rdrs-flash.js:75` 的 `redirect()` 會先寫 flash cookie，再用 `window.location.href = url`。目前 feeds 頁多個成功流程會呼叫它：

- 新增 feed：`static/js/pages/feeds.js:343`
- 更新 feed：`static/js/pages/feeds.js:423`
- 刪除 feed：`static/js/pages/feeds.js:439`
- refresh feed：`static/js/pages/feeds.js:462`
- 匯入 OPML：`static/js/pages/feeds.js:514`

另外 admin masquerade、logout 等 auth/session 切換也會走 redirect 或 reload：

- `static/js/pages/admin.js:156`
- `static/js/components/rdrs-sidebar.js:175`
- `static/js/components/rdrs-sidebar.js:190`

如果目標定義是「頁面切換 link / keyboard / dropdown 都走 CSR」，這些 post-action reload 可能是可接受的刻意行為。若目標是「UI 初次載入後，站內互動都不應 full reload」，這些就仍是遺漏點。

相關位置：

- `static/js/components/rdrs-flash.js:75`
- `static/js/components/rdrs-flash.js:77`
- `static/js/pages/feeds.js:343`
- `static/js/pages/feeds.js:423`
- `static/js/pages/feeds.js:439`
- `static/js/pages/feeds.js:462`
- `static/js/pages/feeds.js:514`
- `static/js/pages/admin.js:156`
- `static/js/components/rdrs-sidebar.js:175`
- `static/js/components/rdrs-sidebar.js:190`

## 建議補測

建議在 `e2e/tests/spa-router.spec.ts` 新增至少兩個 document-load tracker 測試：

1. 開啟文章後進入 `/entries/{id}?origin=...`，再切到另一個 SPA route，Back / Forward 回文章 URL 時 document load count 應為 0。
2. `/statistics` 自訂日期 Apply 後 document load count 應為 0，且頁面資料與 URL query 正確更新。

