//! `ac2 artnet target|fade|test`: where the rig's Art-Net universe goes and how it moves.

use ac2_client::expect_body;
use ac2_proto::model::{ArtnetFade, ArtnetTarget, Rig, dmx_list_text};
use ac2_proto::units::Seconds;
use ac2_proto::{Command, ReplyBody};
use serde_json::json;

use super::osc::rig_edit;
use super::{connect, state};
use crate::CliError;
use crate::args::*;
use crate::output::Out;

pub(crate) async fn artnet(cli: &Cli, cmd: &ArtnetCmd, out: &mut Out<'_>) -> Result<(), CliError> {
    match cmd {
        ArtnetCmd::Target { addr, universe } => {
            let target = match (addr.0, universe) {
                (None, None) => None,
                (None, Some(_)) => {
                    return Err(CliError::Usage(
                        "`off` takes no universe: ac2 artnet target off".into(),
                    ));
                }
                (Some(_), None) => {
                    return Err(CliError::Usage(
                        "give the universe too: ac2 artnet target <ip[:port]> <N|net.sub.uni>"
                            .into(),
                    ));
                }
                (Some(addr), Some(u)) => Some(ArtnetTarget {
                    addr,
                    universe: u.0,
                }),
            };
            let c = connect(cli, false).await?;
            let rig = rig_edit(&c, |base| Command::ArtnetTarget { base, target }).await?;
            out.emit(&rig, || match rig.artnet_target {
                Some(t) => format!("Art-Net universe {} goes to {}", universe_text(&t), t.addr),
                None => "Art-Net target off: no frames are sent".to_owned(),
            })?;
        }
        ArtnetCmd::Fade { seconds } => {
            let c = connect(cli, false).await?;
            let seconds = Seconds(seconds.0.seconds());
            let rig = rig_edit(&c, |base| Command::ArtnetFade { base, seconds }).await?;
            out.emit(&rig, || match fade_text(rig.artnet_fade) {
                t if rig.artnet_fade.seconds() == 0.0 => {
                    format!("Art-Net fade {t}: cue changes jump")
                }
                t => format!("Art-Net fade {t}: cue changes fade over it"),
            })?;
        }
        ArtnetCmd::Test { channels } => {
            let c = connect(cli, false).await?;
            let r = c
                .call(Command::ArtnetTest {
                    channels: channels.clone(),
                })
                .await?;
            let rev = expect_body!("artnet.test", r, ReplyBody::Ack { rev } => rev)?;
            let rig = state(&c).await?.rig;
            out.emit(
                &json!({ "rev": rev, "sent": channels, "artnet": rig.artnet }),
                || {
                    let to = rig.artnet_target.map_or_else(
                        || "?".to_owned(),
                        |t| format!("universe {} at {}", universe_text(&t), t.addr),
                    );
                    let mut s = format!(
                        "sent {} to {to} (held 5 s, then the cues' frame)",
                        dmx_list_text(channels)
                    );
                    if let Some(e) = &rig.artnet.last_error {
                        s.push_str(&format!("\nlast error: {e}"));
                    }
                    s
                },
            )?;
        }
    }
    Ok(())
}

/// A universe as both numbering schemes say it: `0.0.1 (1)`.
fn universe_text(t: &ArtnetTarget) -> String {
    format!("{} ({})", t.universe, t.universe.get())
}

/// A fade as `rig status` and `artnet fade` say it: `1.5 s`.
fn fade_text(f: ArtnetFade) -> String {
    format!("{} s", f.seconds())
}

/// The Art-Net lines of `rig status`.
pub(crate) fn status_text(rig: &Rig) -> String {
    let lit = if rig.dmx.is_empty() {
        "none".to_owned()
    } else {
        dmx_list_text(&rig.dmx)
    };
    format!(
        "Art-Net target: {}\nfade: {}\nsent: {}\nlast error: {}\nchannels above 0: {lit}\n",
        rig.artnet_target.map_or_else(
            || "off".to_owned(),
            |t| format!("{} universe {}", t.addr, universe_text(&t))
        ),
        fade_text(rig.artnet_fade),
        rig.artnet.sent,
        rig.artnet.last_error.as_deref().unwrap_or("none"),
    )
}
