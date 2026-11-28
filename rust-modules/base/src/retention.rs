//! How long data obtained from TMDB may stay on the device.
//!
//! TMDB's API terms (section 1.C, https://www.themoviedb.org/api-terms-of-use) forbid caching
//! "any information obtained through or from TMDB or the TMDB APIs" for longer than six months. The
//! two TMDB-derived stores, the Seerr answers (`plex::seerr_cache`) and the artwork cache's
//! `image.tmdb.org` pictures (`platform::imgcache`), both read this one limit. It is half the
//! six months so a skewed clock or a copy refreshed late still falls well inside them.

/// Seconds a TMDB-derived entry may be served or kept, measured from when it was fetched.
pub const TMDB_MAX_AGE_SECS: u64 = 90 * 24 * 60 * 60;
