use phoenix_core::domain::instance_identity::InstanceId;
use sqlx::{Sqlite, Transaction};

use crate::{
    federation_enrollment::require_federation_send_authority_tx, DbResult, FederationSendAuthority,
};

/// Native conversation actions use the same authenticated enrollment as federation send.
#[derive(Clone, Debug)]
pub struct FederationConversationAuthority(FederationSendAuthority);

impl From<FederationSendAuthority> for FederationConversationAuthority {
    fn from(authority: FederationSendAuthority) -> Self {
        Self(authority)
    }
}

impl FederationConversationAuthority {
    #[must_use]
    pub fn caller_instance_id(&self) -> InstanceId {
        self.0.caller_instance_id()
    }

    pub(crate) async fn require_tx(&self, tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
        require_federation_send_authority_tx(tx, &self.0).await
    }
}

#[cfg(test)]
pub(crate) async fn enroll_test_peer(
    db: &crate::Database,
    caller: InstanceId,
    token: &[u8],
) -> FederationConversationAuthority {
    let verifier =
        phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(token);
    db.replace_federation_enrollment(caller, &"native peer".parse().unwrap(), &verifier)
        .await
        .unwrap();
    FederationSendAuthority::new(caller, verifier).into()
}
