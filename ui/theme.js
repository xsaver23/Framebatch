'use strict';
// Applies the saved light/dark choice before the page draws (dark is the default, as in
// most photo editors). A separate file because the app's security policy blocks inline scripts.
(() => { let theme = 'dark'; try { if (localStorage.getItem('theme') === 'light') theme = 'light'; } catch { /* storage unavailable */ } document.documentElement.dataset.theme = theme; })();
