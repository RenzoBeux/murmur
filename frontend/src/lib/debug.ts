/**
 * Dev-only logger for hot paths (per-transcript-segment, per-poll). In production
 * builds Next.js inlines NODE_ENV, so these calls compile to a no-op and the logged
 * objects are never retained by the WebView console.
 */
export const debugLog: typeof console.log =
  process.env.NODE_ENV === 'development' ? console.log.bind(console) : () => {};
