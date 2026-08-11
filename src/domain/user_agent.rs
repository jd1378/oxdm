//! The User-Agent oxdm sends, and the rule that picks it.
//!
//! Three layers can name one, and only one reaches the wire:
//!
//! 1. the download's own `User-Agent` header (a browser capture brings
//!    the browser's UA with it, so an anti-leech host sees the same
//!    client that was on the page);
//! 2. the global override typed in Settings;
//! 3. this default.
//!
//! "Randomize the User-Agent" sits between 2 and 3: it applies only
//! when nobody typed one, because an explicit UA the user chose must
//! not be shuffled out from under them (odl agrees — it ignores
//! randomization whenever an explicit UA is set).
//!
//! Before this existed, layer 3 was *nothing*: reqwest sends no
//! User-Agent unless told to, so oxdm's own requests arrived
//! unidentified and hosts that reject unknown clients rejected them
//! without a name to complain about.

/// `oxdm/<version>` — a bare product token, the shape curl uses.
///
/// No platform comment: the operating system and architecture describe
/// the user's machine, and they change nothing about what a server
/// sends back for a URL the user picked. What identifies this client
/// is the name and version, which cannot be dropped without lying
/// about who is calling. No browser tokens either — claiming to be
/// Chrome is what the per-download override and the randomizer are
/// for, and a default that lies makes every server log wrong.
pub const fn default_user_agent() -> &'static str {
    concat!("oxdm/", env!("CARGO_PKG_VERSION"))
}

/// The same identity with a purpose comment, for the requests oxdm
/// makes on its own behalf rather than a user's — the update check,
/// and anything like it.
///
/// Built from `default_user_agent`, never spelled out again: a second
/// hand-written `oxdm/{version}` is a version that stops matching the
/// app the day someone forgets it.
pub fn app_user_agent(purpose: &str) -> String {
    format!("{} ({purpose})", default_user_agent())
}

/// The UA the global layer contributes: an explicit override if the
/// user typed one, `None` when they asked for randomization and left
/// the field empty (odl picks one per request), the app default
/// otherwise.
///
/// Single source of truth for both the wire
/// (`data::mapping::settings_to_download_options`) and the display
/// (`job::will_send_headers`) — they must never disagree about what
/// the next request carries.
pub fn effective_user_agent(s: &super::Settings) -> Option<String> {
    if let Some(ua) = s
        .user_agent
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        return Some(ua.to_owned());
    }
    // A `User-Agent` typed into the global header table is an explicit
    // choice too — and it has to be lifted here, because reqwest
    // applies the UA option *after* the default headers and would
    // otherwise overwrite it with our default.
    if let Some((_, ua)) = s
        .headers
        .iter()
        .find(|(k, _)| super::header_name_eq(k, "User-Agent"))
        .filter(|(_, v)| !v.trim().is_empty())
    {
        return Some(ua.clone());
    }
    (!s.randomize_user_agent).then(|| default_user_agent().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Settings;

    /// Name and version, and nothing about the machine underneath.
    #[test]
    fn the_default_names_the_app_and_its_version_only() {
        let ua = default_user_agent();
        assert_eq!(ua, format!("oxdm/{}", env!("CARGO_PKG_VERSION")));
        assert!(!ua.contains('('), "no platform comment: {ua}");
    }

    /// Every UA oxdm sends is the crate version. A hand-written one
    /// somewhere else is a version that stops matching the app on the
    /// next release, and nothing fails until a server logs it.
    #[test]
    fn the_purpose_variant_is_built_from_the_default() {
        let ua = app_user_agent("update-check");
        assert_eq!(ua, format!("{} (update-check)", default_user_agent()));
        assert!(ua.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn a_typed_agent_outranks_randomization() {
        let s = Settings {
            user_agent: Some("curl/8.5.0".into()),
            randomize_user_agent: true,
            ..Settings::default()
        };
        assert_eq!(effective_user_agent(&s).as_deref(), Some("curl/8.5.0"));
    }

    #[test]
    fn randomization_only_applies_to_an_empty_field() {
        let mut s = Settings {
            randomize_user_agent: true,
            ..Settings::default()
        };
        assert_eq!(effective_user_agent(&s), None);
        s.user_agent = Some("   ".into());
        assert_eq!(effective_user_agent(&s), None, "blank is not a choice");
    }

    #[test]
    fn otherwise_every_request_still_carries_a_name() {
        let s = Settings::default();
        assert_eq!(
            effective_user_agent(&s).as_deref(),
            Some(default_user_agent())
        );
    }
}
