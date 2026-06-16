//! Kill switch with pf. Rules go into our own anchor; pf is enabled with a
//! reference token (`pfctl -E`), so it is turned off again only if nobody
//! else needs it.
//!
//! The anchor survives a crash of the service (fail-closed) until it restarts
//! or the machine reboots.

use super::exec;
use crate::macos_text::parse_pf_token;
use crate::{FirewallPolicy, NetError, Result, pf};

/// Our pf anchor, evaluated by the stock /etc/pf.conf through `anchor "com.apple/*"`.
const ANCHOR: &str = "com.apple/submarine";

/// pf backend of the kill switch.
pub struct Firewall {
    // reference token of `pfctl -E`, held while our rules need pf enabled
    token: Option<String>,
}

impl Firewall {
    /// Creates the firewall; nothing is loaded until [`Firewall::apply`].
    pub fn new() -> Self {
        Self { token: None }
    }

    /// Loads the rules for `policy` into our anchor, enabling pf if needed, or
    /// removes them when nothing must be blocked.
    pub async fn apply(&mut self, policy: &FirewallPolicy) -> Result<()> {
        let Some(rules) = pf::render(policy) else {
            return self.reset().await;
        };
        // pf is enabled once, keeping the token that releases our reference
        if self.token.is_none() {
            let (out, err) = exec("pfctl", &["-E"], None).await?;
            let token =
                parse_pf_token(&format!("{out}\n{err}")).ok_or_else(|| NetError::Command {
                    command: "pfctl -E".into(),
                    stderr: "no reference token in the output".into(),
                })?;
            self.token = Some(token);
        }
        // the ruleset is fed on stdin and replaces the whole anchor
        exec("pfctl", &["-a", ANCHOR, "-f", "-"], Some(&rules)).await?;
        tracing::info!(allow_lan = policy.allow_lan, "firewall applied");
        Ok(())
    }

    /// Flushes our anchor and releases our reference to pf.
    pub async fn reset(&mut self) -> Result<()> {
        // also clears rules left by a crashed run
        let _ = exec("pfctl", &["-a", ANCHOR, "-F", "all"], None).await;
        // pf is disabled only if no other reference holder needs it
        if let Some(token) = self.token.take() {
            exec("pfctl", &["-X", &token], None).await?;
        }
        Ok(())
    }
}

impl Default for Firewall {
    fn default() -> Self {
        Self::new()
    }
}
