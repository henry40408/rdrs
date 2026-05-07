// static/js/router.js
// SPA router — intercepts internal-link clicks and swaps the page element
// in place instead of triggering a full document reload.
//
// Loaded by app_shell.html after the page-module script. The first paint is
// still server-rendered (handler returns the shell with element_tag +
// script_path); the router takes over from there.
//
// Filled in by the next commit. This commit just lands the file so the
// shell's <script> tag has something to load.

console.debug('[rdrs-router] loaded');
