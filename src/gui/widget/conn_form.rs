//! Proxy and site-authentication form.
//!
//! Two dialogs let a user say how a download reaches its server: the
//! Add window's Advanced pane and Properties → Connection. They used to
//! say it in different vocabularies — Add offered "None / HTTP /
//! SOCKS5" and a bare username+password pair, Properties offered five
//! proxy modes, a proxy-authentication switch, and three auth schemes —
//! so the same download meant different things depending on which
//! window created it.
//!
//! The controls, the state they edit, the validation, and the wording
//! live here once. A window supplies its own chrome (where the sections
//! sit, how wide, what else is on the page) and its own message
//! constructors; it does not get to invent a different proxy model.

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::domain::{AuthAdv, AuthScheme, Creds, ProxyAdv, ProxyMode};
use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::{
    Btn, BtnSize, TextInput, hairline, labeled_section, segmented, toggle_row,
};

/// Proxy-mode options. Values and labels stay index-aligned.
///
/// What each one means, and why the list is exactly this long:
/// `Inherit` sets no per-job override (a legacy `Job.proxy` URL still
/// applies under it), `System` clears the global proxy so the standard
/// environment variables decide, `None` asks for a direct connection,
/// and the three explicit modes name a server. Digest-style guesswork
/// is not on offer anywhere: a mode is here only if the data layer can
/// carry it out.
pub const PROXY_MODE_VALUES: &[ProxyMode] = &[
    ProxyMode::Inherit,
    ProxyMode::None,
    ProxyMode::System,
    ProxyMode::Http,
    ProxyMode::Https,
    ProxyMode::Socks5,
];
pub const PROXY_MODE_LABELS: &[(&str, Option<&str>)] = &[
    ("Inherit", None),
    ("None", None),
    ("System", None),
    ("HTTP", None),
    ("HTTPS", None),
    ("SOCKS5", None),
];

/// Site-auth schemes. Digest is deliberately absent (no odl/reqwest
/// implementation); a legacy persisted `Digest` is displayed as None.
pub const AUTH_SCHEME_VALUES: &[AuthScheme] =
    &[AuthScheme::None, AuthScheme::Basic, AuthScheme::Bearer];
pub const AUTH_SCHEME_LABELS: &[(&str, Option<&str>)] =
    &[("None", None), ("HTTP Basic", None), ("Bearer token", None)];

/// Width of the proxy port field (design `.prop-proxy-port` ≈ 90px).
const PORT_INPUT_W: f32 = 90.0;

/// The proxy half of the form, as the user is editing it.
///
/// `Default` is the stored default, not the zeroed one — remote DNS is
/// on for a fresh SOCKS5 proxy, the same as `ProxyAdv::default`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyForm {
    pub mode: ProxyMode,
    pub host: String,
    pub port: String,
    pub auth_enabled: bool,
    pub username: String,
    pub password: String,
    /// The password field was typed in this session. Empty *and*
    /// edited is the only way to say "delete the stored secret" — a
    /// stored one never round-trips into the form, so an untouched
    /// empty field means "keep it".
    pub password_edited: bool,
    pub remote_dns: bool,
}

impl Default for ProxyForm {
    fn default() -> Self {
        Self::from_adv(&ProxyAdv::default())
    }
}

impl ProxyForm {
    pub fn from_adv(p: &ProxyAdv) -> Self {
        Self {
            mode: p.mode,
            host: p.host.clone(),
            port: p.port.clone(),
            auth_enabled: p.auth_enabled,
            username: p.username.clone(),
            // Secrets never come back down to a form.
            password: String::new(),
            password_edited: false,
            remote_dns: p.remote_dns,
        }
    }

    pub fn to_adv(&self) -> ProxyAdv {
        ProxyAdv {
            mode: self.mode,
            host: self.host.trim().to_owned(),
            port: self.port.trim().to_owned(),
            auth_enabled: self.auth_enabled,
            username: self.username.trim().to_owned(),
            password: self.password.clone(),
            clear_password: self.password_edited && self.password.is_empty(),
            remote_dns: self.remote_dns,
            bypass: String::new(),
        }
    }

    /// A mode that names a server, and so needs host and port.
    pub fn explicit(&self) -> bool {
        matches!(
            self.mode,
            ProxyMode::Http | ProxyMode::Https | ProxyMode::Socks5
        )
    }

    pub fn host_invalid(&self) -> bool {
        self.explicit() && self.host.trim().is_empty()
    }

    /// Empty counts: the data layer has no fallback port, so a blank
    /// one only fails at job start.
    pub fn port_invalid(&self) -> bool {
        self.explicit() && !self.port.trim().parse::<u16>().is_ok_and(|p| p >= 1)
    }

    pub fn invalid(&self) -> bool {
        self.host_invalid() || self.port_invalid()
    }
}

/// The site-authentication half.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthForm {
    pub scheme: AuthScheme,
    pub username: String,
    pub password: String,
    pub token: String,
    /// See `ProxyForm::password_edited` — covers whichever secret the
    /// current scheme uses; both land on the same encrypted column.
    pub secret_edited: bool,
}

impl AuthForm {
    /// `username` is the job's own `auth_user` column, which outranks
    /// the blob's copy: it is what the runner builds Basic credentials
    /// from.
    pub fn from_adv(a: &AuthAdv, username: Option<&str>) -> Self {
        let stored_user = username.filter(|u| !u.trim().is_empty());
        Self {
            // Digest was never implemented; showing it would promise
            // something no request can carry out.
            scheme: match a.scheme {
                AuthScheme::Digest => AuthScheme::None,
                // A job added before the scheme existed carries its
                // credentials on the username column with the blob
                // still at its `None` default. The runner sends Basic
                // for it, so the form has to say Basic — otherwise the
                // dialog offers to "turn off" auth that is already on.
                AuthScheme::None if stored_user.is_some() => AuthScheme::Basic,
                s => s,
            },
            username: stored_user.unwrap_or(&a.username).to_owned(),
            password: String::new(),
            token: String::new(),
            secret_edited: false,
        }
    }

    pub fn to_adv(&self) -> AuthAdv {
        let secret_empty = match self.scheme {
            AuthScheme::Bearer => self.token.is_empty(),
            _ => self.password.is_empty(),
        };
        AuthAdv {
            scheme: self.scheme,
            username: self.username.trim().to_owned(),
            password: self.password.clone(),
            token: self.token.clone(),
            clear_secret: self.secret_edited && secret_empty,
        }
    }
}

/// Both halves as the daemon wants them.
pub fn creds(proxy: &ProxyForm, auth: &AuthForm) -> Creds {
    Creds {
        proxy: proxy.to_adv(),
        auth: auth.to_adv(),
    }
}

/// What the window knows that the form does not.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormCtx<'a> {
    /// False while the job is running and its connection settings are
    /// locked: the controls render read-only instead of vanishing.
    pub editable: bool,
    /// A secret is already stored for this field, so an empty input
    /// means "keep it" and there is something to offer removing.
    pub stored_secret: bool,
    /// The legacy per-job proxy URL, which still wins under Inherit.
    /// Surfaced rather than hidden — it is the reason a job with
    /// "Inherit" selected can still be going through a proxy.
    pub legacy_url: Option<&'a str>,
}

/// Message constructors the host window owns.
pub struct ProxyMsgs<M> {
    /// Index into `PROXY_MODE_VALUES`.
    pub mode: fn(usize) -> M,
    pub host: fn(String) -> M,
    pub port: fn(String) -> M,
    pub auth_enabled: fn(bool) -> M,
    pub username: fn(String) -> M,
    pub password: fn(String) -> M,
    pub password_clear: M,
    pub remote_dns: fn(bool) -> M,
}

pub struct AuthMsgs<M> {
    /// Index into `AUTH_SCHEME_VALUES`.
    pub scheme: fn(usize) -> M,
    pub username: fn(String) -> M,
    pub password: fn(String) -> M,
    pub token: fn(String) -> M,
    pub secret_clear: M,
}

/// Honest per-mode explanation. Every mode that behaves in a way the
/// segmented control's one-word label cannot convey gets a sentence;
/// the explicit ones are self-explanatory and get none.
fn mode_hint(st: &ProxyForm, ctx: FormCtx<'_>) -> Option<String> {
    mode_hint_for(st, ctx)
}

/// The hint a window needs before it lays anything out, so it can size
/// itself around the paragraph this will render.
pub fn mode_hint_text(st: &ProxyForm) -> Option<String> {
    mode_hint_for(st, FormCtx::default())
}

fn mode_hint_for(st: &ProxyForm, ctx: FormCtx<'_>) -> Option<String> {
    match st.mode {
        ProxyMode::Inherit => {
            let mut hint = "Inherit (global / environment): uses the proxy from \
                            Settings → Network, or your proxy environment variables."
                .to_owned();
            if let Some(legacy) = ctx.legacy_url {
                hint.push_str(&format!("\nThis job carries a proxy URL: {legacy}"));
            }
            Some(hint)
        }
        ProxyMode::System => Some(
            "System (environment variables): ignores the global oxdm proxy for this \
             job; the standard proxy environment variables still apply."
                .to_owned(),
        ),
        ProxyMode::None => Some(
            "None (direct): connects straight to the server, ignoring the global \
             proxy, your proxy environment variables and the system one."
                .to_owned(),
        ),
        _ => None,
    }
}

/// The proxy section: mode, its explanation, and — for an explicit
/// mode — server, authentication and SOCKS5 DNS.
pub fn proxy_section<'a, M: Clone + 'a>(
    t: &Tokens,
    st: &'a ProxyForm,
    ctx: FormCtx<'_>,
    msgs: ProxyMsgs<M>,
) -> Element<'a, M> {
    let editable = ctx.editable;
    let mode_idx = PROXY_MODE_VALUES
        .iter()
        .position(|m| *m == st.mode)
        .unwrap_or(0);
    let mode_row = column![
        text("Use proxy")
            .font(theme::BODY_MEDIUM)
            .size(12.0)
            .color(t.fg_1),
        text(
            "Route this download's traffic through a proxy server. Overrides the \
             global setting in Settings → Network."
        )
        .font(theme::BODY)
        .size(11.0)
        .color(t.fg_3),
        if editable {
            segmented(t, PROXY_MODE_LABELS, mode_idx, BtnSize::Sm, msgs.mode)
        } else {
            // Locked: render the selection read-only.
            text(PROXY_MODE_LABELS[mode_idx].0)
                .font(theme::BODY_MEDIUM)
                .size(12.0)
                .color(t.fg_2)
                .into()
        },
    ]
    .spacing(6.0);

    let mut body = column![container(mode_row).padding([10.0, theme::space::S3])];
    if let Some(hint) = mode_hint(st, ctx) {
        body = body.push(
            container(
                text(hint)
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3)
                    .line_height(iced::widget::text::LineHeight::Relative(1.5)),
            )
            .padding(iced::Padding {
                left: theme::space::S3,
                right: theme::space::S3,
                bottom: 10.0,
                ..Default::default()
            }),
        );
    }

    if st.explicit() {
        let socks5 = st.mode == ProxyMode::Socks5;
        let mut server = column![
            text("Server")
                .font(theme::BODY_MEDIUM)
                .size(12.0)
                .color(t.fg_1),
            row![
                TextInput::new(&st.host)
                    .hint("proxy.example.com")
                    .mono()
                    .enabled(editable)
                    .on_input(msgs.host)
                    .view(t),
                text(":").font(theme::MONO).size(12.0).color(t.fg_3),
                TextInput::new(&st.port)
                    .hint(if socks5 { "1080" } else { "8080" })
                    .mono()
                    .width(Length::Fixed(PORT_INPUT_W))
                    .enabled(editable)
                    .on_input(msgs.port)
                    .view(t),
            ]
            .spacing(6.0)
            .align_y(Alignment::Center),
        ]
        .spacing(6.0);
        // Both fields are required — the data layer has no fallback for
        // either, so an explicit mode with a blank one only fails at
        // job start.
        let problem = if st.host_invalid() {
            Some("Host is required for an explicit proxy.")
        } else if st.port.trim().is_empty() {
            Some("Port is required for an explicit proxy.")
        } else if st.port_invalid() {
            Some("Port must be between 1 and 65535.")
        } else {
            None
        };
        if let Some(problem) = problem {
            server = server.push(
                row![
                    icons::icon("triangle-alert", 10.0, t.status_danger),
                    text(problem)
                        .font(theme::BODY_MEDIUM)
                        .size(10.5)
                        .color(t.status_danger),
                ]
                .spacing(4.0)
                .align_y(Alignment::Center),
            );
        }
        body = body
            .push(hairline(t.border_subtle))
            .push(container(server).padding([10.0, theme::space::S3]))
            .push(hairline(t.border_subtle))
            .push(toggle_row(
                t,
                "Proxy authentication",
                "Username and password sent to the proxy itself (not the destination).",
                st.auth_enabled,
                editable,
                msgs.auth_enabled,
            ));
        if st.auth_enabled {
            let mut creds = column![
                row![
                    TextInput::new(&st.username)
                        .hint("username")
                        .enabled(editable)
                        .on_input(msgs.username)
                        .view(t),
                    TextInput::new(&st.password)
                        .hint(if ctx.stored_secret {
                            "(unchanged)"
                        } else {
                            "password"
                        })
                        .secure(true)
                        .enabled(editable)
                        .on_input(msgs.password)
                        .view(t),
                ]
                .spacing(theme::space::S2)
            ]
            .spacing(6.0);
            if ctx.stored_secret {
                creds = creds.push(stored_secret_row(
                    t,
                    editable,
                    st.password_edited && st.password.is_empty(),
                    msgs.password_clear,
                ));
            }
            body = body.push(container(creds).padding([10.0, theme::space::S3]));
        }
        if socks5 {
            body = body.push(hairline(t.border_subtle)).push(toggle_row(
                t,
                "Resolve DNS through proxy",
                "Send hostname lookups through the SOCKS5 server. Hides DNS queries \
                 from your local resolver.",
                st.remote_dns,
                editable,
                msgs.remote_dns,
            ));
        }
    }
    labeled_section(t, "proxy", body.into())
}

/// The site-authentication section: scheme, then whatever it needs.
pub fn auth_section<'a, M: Clone + 'a>(
    t: &Tokens,
    st: &'a AuthForm,
    ctx: FormCtx<'_>,
    msgs: AuthMsgs<M>,
) -> Element<'a, M> {
    let editable = ctx.editable;
    let scheme_idx = AUTH_SCHEME_VALUES
        .iter()
        .position(|s| *s == st.scheme)
        .unwrap_or(0);
    let mut body = column![
        container(
            column![
                text("Scheme")
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                text("Sent to the destination server, not the proxy.")
                    .font(theme::BODY)
                    .size(11.0)
                    .color(t.fg_3),
                if editable {
                    segmented(t, AUTH_SCHEME_LABELS, scheme_idx, BtnSize::Sm, msgs.scheme)
                } else {
                    text(AUTH_SCHEME_LABELS[scheme_idx].0)
                        .font(theme::BODY_MEDIUM)
                        .size(12.0)
                        .color(t.fg_2)
                        .into()
                },
            ]
            .spacing(6.0)
        )
        .padding([10.0, theme::space::S3]),
    ];
    match st.scheme {
        AuthScheme::Basic => {
            let mut creds = column![
                row![
                    TextInput::new(&st.username)
                        .hint("username")
                        .enabled(editable)
                        .on_input(msgs.username)
                        .view(t),
                    TextInput::new(&st.password)
                        .hint(if ctx.stored_secret {
                            "(unchanged)"
                        } else {
                            "password"
                        })
                        .secure(true)
                        .enabled(editable)
                        .on_input(msgs.password)
                        .view(t),
                ]
                .spacing(theme::space::S2)
            ]
            .spacing(6.0);
            if ctx.stored_secret {
                creds = creds.push(stored_secret_row(
                    t,
                    editable,
                    st.secret_edited && st.password.is_empty(),
                    msgs.secret_clear,
                ));
            }
            body = body
                .push(hairline(t.border_subtle))
                .push(container(creds).padding([10.0, theme::space::S3]));
        }
        AuthScheme::Bearer => {
            let mut field = column![
                text("Token")
                    .font(theme::BODY_MEDIUM)
                    .size(12.0)
                    .color(t.fg_1),
                TextInput::new(&st.token)
                    .hint(if ctx.stored_secret {
                        "(unchanged)"
                    } else {
                        "eyJhbGciOi…"
                    })
                    .mono()
                    .secure(true)
                    .enabled(editable)
                    .on_input(msgs.token)
                    .view(t),
            ]
            .spacing(6.0);
            if ctx.stored_secret {
                field = field.push(stored_secret_row(
                    t,
                    editable,
                    st.secret_edited && st.token.is_empty(),
                    msgs.secret_clear,
                ));
            }
            body = body
                .push(hairline(t.border_subtle))
                .push(container(field).padding([10.0, theme::space::S3]));
        }
        _ => {}
    }
    labeled_section(t, "site authentication", body.into())
}

/// Footer for a secret input whose stored value never round-trips into
/// the form. Without it "delete the stored secret" would be an
/// invisible gesture — type into an already-empty field, then erase it
/// — so the state and the way out are both spelled out.
pub fn stored_secret_row<'a, M: Clone + 'a>(
    t: &Tokens,
    editable: bool,
    pending_clear: bool,
    clear: M,
) -> Element<'a, M> {
    if pending_clear {
        return row![
            icons::icon("triangle-alert", 11.0, t.status_danger),
            text("Stored secret will be removed on Apply.")
                .font(theme::BODY)
                .size(11.0)
                .color(t.status_danger),
        ]
        .spacing(4.0)
        .align_y(Alignment::Center)
        .into();
    }
    row![
        icons::icon("lock", 11.0, t.status_success),
        text("Stored (encrypted). Leave blank to keep it.")
            .font(theme::BODY)
            .size(11.0)
            .color(t.fg_3),
        iced::widget::Space::new().width(Length::Fill),
        Btn::new("Remove")
            .toolbar()
            .icon("trash-2")
            .size(BtnSize::Sm)
            .enabled(editable)
            .on_press(clear)
            .view(t),
    ]
    .spacing(4.0)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_secret_survives_an_untouched_field() {
        let mut p = ProxyForm {
            mode: ProxyMode::Http,
            host: "proxy.example.com".into(),
            port: "8080".into(),
            auth_enabled: true,
            username: "u".into(),
            ..Default::default()
        };
        assert!(!p.to_adv().clear_password, "untouched means keep");
        p.password_edited = true;
        assert!(p.to_adv().clear_password, "emptied on purpose means delete");
        p.password = "s3cret".into();
        assert!(!p.to_adv().clear_password, "a new one replaces it");
    }

    /// Bearer's secret is the token, Basic's is the password — the
    /// clear flag has to watch whichever the scheme uses, or switching
    /// scheme would silently delete the stored one.
    #[test]
    fn the_clear_flag_follows_the_scheme() {
        let mut a = AuthForm {
            scheme: AuthScheme::Bearer,
            secret_edited: true,
            password: "left over from Basic".into(),
            ..Default::default()
        };
        assert!(a.to_adv().clear_secret, "the token is what is empty");
        a.token = "t".into();
        assert!(!a.to_adv().clear_secret);
    }

    #[test]
    fn an_explicit_mode_needs_a_host_and_a_port() {
        let mut p = ProxyForm {
            mode: ProxyMode::Socks5,
            ..Default::default()
        };
        assert!(p.host_invalid() && p.port_invalid());
        p.host = "h".into();
        p.port = "0".into();
        assert!(p.port_invalid(), "0 is not a port");
        p.port = "65536".into();
        assert!(p.port_invalid());
        p.port = "1080".into();
        assert!(!p.invalid());

        // Nothing to validate when no server was named.
        p.mode = ProxyMode::Inherit;
        p.host.clear();
        p.port.clear();
        assert!(!p.invalid());
    }

    /// A job from before the scheme selector existed keeps working:
    /// the runner sends Basic off the username column, so the form
    /// must not show "None" over credentials that are in use.
    #[test]
    fn a_legacy_basic_job_reads_as_basic() {
        let adv = AuthAdv::default();
        assert_eq!(
            AuthForm::from_adv(&adv, Some("someone")).scheme,
            AuthScheme::Basic
        );
        assert_eq!(
            AuthForm::from_adv(&adv, Some("  ")).scheme,
            AuthScheme::None
        );
        assert_eq!(AuthForm::from_adv(&adv, None).scheme, AuthScheme::None);
    }

    #[test]
    fn digest_is_never_shown() {
        let adv = AuthAdv {
            scheme: AuthScheme::Digest,
            ..Default::default()
        };
        assert_eq!(AuthForm::from_adv(&adv, None).scheme, AuthScheme::None);
    }

    #[test]
    fn the_mode_lists_stay_aligned() {
        assert_eq!(PROXY_MODE_VALUES.len(), PROXY_MODE_LABELS.len());
        assert_eq!(AUTH_SCHEME_VALUES.len(), AUTH_SCHEME_LABELS.len());
    }
}
