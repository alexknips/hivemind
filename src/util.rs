use crate::error::CommandError;
use crate::Result;

pub(crate) fn require_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

/// Every `actor_id` in HiveMind is a typed, human-readable id (`human:<name>`,
/// `agent:<tool>:<name>`, `slack:<team>:<user>`, ...) -- never a bare UUID. A bare
/// UUID reaching an actor field means a raw session/message id leaked in where a
/// stable name belongs (hivemind-zdsh.9): reject it here, at the write layer,
/// instead of letting it silently corrupt provenance until a later read fails.
pub(crate) fn require_valid_actor_id(actor_id: &str) -> Result<()> {
    require_non_empty("actor_id", actor_id)?;
    if uuid::Uuid::parse_str(actor_id.trim()).is_ok() {
        return Err(CommandError::Validation(format!(
            "actor_id must not be a bare UUID ({actor_id}) -- use a typed id like human:<name> or agent:<tool>:<name>"
        ))
        .into());
    }
    Ok(())
}

/// The agent-token provisioning path (hivemind-zdsh.19) mints identities directly,
/// with no `hm_users` row to derive an email from -- unlike `create_user`/
/// `mint_user_token`, which always produce `human:<email>`. Require the
/// `agent:<tool>:<name>` shape here so that path can't be used to mint a
/// human-shaped or otherwise untyped identity.
pub(crate) fn require_agent_actor_id(actor_id: &str) -> Result<()> {
    require_valid_actor_id(actor_id)?;
    let mut parts = actor_id.trim().splitn(3, ':');
    let scheme = parts.next().unwrap_or_default();
    let tool = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if scheme != "agent" || tool.is_empty() || name.is_empty() {
        return Err(CommandError::Validation(format!(
            "actor_id must be shaped agent:<tool>:<name> ({actor_id})"
        ))
        .into());
    }
    Ok(())
}
