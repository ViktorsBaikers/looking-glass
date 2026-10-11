// Still an empty CSR shell (ssr is off in the layout), but a prerendered one
// lists this page's chunks as modulepreloads, so the browser fetches them with
// the entry instead of discovering them after it runs.
export const prerender = true;
