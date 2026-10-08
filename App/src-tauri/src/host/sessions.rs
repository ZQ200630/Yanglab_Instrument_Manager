#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn remote_attachment_and_revocation_are_bound_to_the_authenticated_peer() {
        let mut book = ClientSessions::new("b".repeat(32));
        let a = book.join_peer(&json!({}), "a").unwrap();
        let params = json!({"attach_token":a.attach_token,"channel":"heartbeat"});
        assert!(book.join_peer(&params, "other").is_err());
        assert!(book.join(&params).is_err());
        let heartbeat = book.join_peer(&params, "a").unwrap();
        assert_eq!(heartbeat.session.id(), a.session.id());
        let local = book.join(&json!({})).unwrap();
        let revoked = book.revoke_peer("a");
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0].id(), a.session.id());
        assert!(!book.active(a.session.id()));
        assert!(book.active(local.session.id()));
        assert!(book.join_peer(&params, "a").is_err());
    }
    #[test]
    fn recovery_fences_only_the_old_authenticated_peer_session() {
        let mut book=ClientSessions::new("b".repeat(32));let old=book.join_peer(&json!({}),"a").unwrap();let other=book.join_peer(&json!({}),"other").unwrap();
        assert!(book.fence_release(old.session.id(),"other").is_err());assert!(book.control_allowed(old.session.id()));
        book.fence_release(old.session.id(),"a").unwrap();assert!(!book.control_allowed(old.session.id()));assert!(!book.active(old.session.id()));
        assert!(book.control_allowed(other.session.id()));assert!(book.join_peer(&json!({"attach_token":old.attach_token,"channel":"safety"}),"a").is_err());
    }
    #[test]
    fn independent_channels_share_only_their_own_authenticated_session() {
        let mut book = ClientSessions::new("b".repeat(32));
        let primary = book.join(&json!({})).unwrap();
        let heartbeat = book
            .join(&json!({"attach_token":primary.attach_token,"channel":"heartbeat"}))
            .unwrap();
        assert_eq!(primary.session.id(), heartbeat.session.id());
        assert!(book
            .join(&json!({"attach_token":"c".repeat(32),"channel":"events"}))
            .is_err());
        assert!(book
            .join(&json!({"attach_token":primary.attach_token,"channel":"heartbeat"}))
            .is_err());
        assert!(book.leave(&heartbeat).is_some());
        assert!(!book.active(primary.session.id()));
    }
    #[test]
    fn safety_has_its_own_authenticated_channel_and_duplicate_is_rejected() {
        let mut book = ClientSessions::new("b".repeat(32));
        let primary = book.join(&json!({})).unwrap();
        let safety = book
            .join(&json!({"attach_token":primary.attach_token,"channel":"safety"}))
            .unwrap();
        assert_eq!(primary.session.id(), safety.session.id());
        assert!(book
            .join(&json!({"attach_token":primary.attach_token,"channel":"safety"}))
            .is_err());
    }
    #[test]
    fn revoked_client_can_close_existing_channels_without_regaining_authority() {
        let mut book = ClientSessions::new("b".repeat(32));
        let control = book.join(&json!({})).unwrap();
        let safety = book
            .join(&json!({"attach_token":control.attach_token,"channel":"safety"}))
            .unwrap();
        book.leave(&control).unwrap();
        assert!(book.begin_close(safety.session.id()).is_ok());
        assert!(!book.control_allowed(safety.session.id()));
        assert!(book
            .join(&json!({"attach_token":safety.attach_token,"channel":"results"}))
            .is_err());
    }
    #[test]
    fn client_capacity_is_logical_not_seven_channels_per_gui() {
        let mut book = ClientSessions::new("b".repeat(32));
        let mut primary = Vec::new();
        for _ in 0..16 {
            let client = book.join(&json!({})).unwrap();
            for channel in ["heartbeat", "events", "results", "safety", "background", "status"] {
                book.join(&json!({"attach_token":client.attach_token,"channel":channel}))
                    .unwrap();
            }
            primary.push(client);
        }
        assert_eq!(book.join(&json!({})).unwrap_err().code, "ClientCapacity");
    }
    #[test]
    fn asynchronous_channels_share_authentication_but_have_distinct_loss_semantics() {
        let mut book = ClientSessions::new("b".repeat(32));
        let primary = book.join(&json!({})).unwrap();
        for channel in ["background", "status"] {
            assert!(book.join(&json!({"attach_token":"c".repeat(32),"channel":channel})).is_err());
        }
        let background = book.join(&json!({"attach_token":primary.attach_token,"channel":"background"})).unwrap();
        let status = book.join(&json!({"attach_token":primary.attach_token,"channel":"status"})).unwrap();
        assert_eq!(primary.session.id(), background.session.id());
        assert_eq!(primary.session.id(), status.session.id());
        assert!(book.join(&json!({"attach_token":primary.attach_token,"channel":"status"})).is_err());
        assert!(book.leave(&status).is_none());
        assert!(book.active(primary.session.id()));
        assert!(book.leave(&background).is_some());
        assert!(!book.control_allowed(primary.session.id()));
    }
}
use super::{contracts::HostError, leases::Session, registry::new_id};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug)]
pub struct ClientChannel {
    pub session: Session,
    pub attach_token: String,
    pub channel: String,
}
struct Group {
    session: Session,
    peer: Option<String>,
    channels: BTreeSet<String>,
    active: bool,
    closing: bool,
}
pub struct ClientSessions {
    boot: String,
    groups: BTreeMap<String, Group>,
}
impl ClientSessions {
    pub fn new(boot: String) -> Self {
        Self {
            boot,
            groups: BTreeMap::new(),
        }
    }
    // Called only after the pipe peer's OS identity has been verified.
    pub fn join(&mut self, params: &Value) -> Result<ClientChannel, HostError> {
        self.join_authenticated(params, None)
    }
    pub fn join_peer(&mut self, params: &Value, peer: &str) -> Result<ClientChannel, HostError> {
        self.join_authenticated(params, Some(peer))
    }
    fn join_authenticated(&mut self, params: &Value, peer: Option<&str>) -> Result<ClientChannel, HostError> {
        if let Some(token) = params.get("attach_token") {
            let token = token
                .as_str()
                .ok_or_else(|| HostError::new("SessionAttach", "Invalid token"))?;
            let channel = params["channel"]
                .as_str()
                .filter(|c| matches!(*c, "heartbeat" | "events" | "results" | "safety" | "background" | "status"))
                .ok_or_else(|| HostError::new("SessionAttach", "Invalid channel"))?;
            let group = self
                .groups
                .get_mut(token)
                .filter(|g| g.active && g.peer.as_deref() == peer)
                .ok_or_else(|| HostError::new("SessionAttach", "Unknown or revoked session"))?;
            if !group.channels.insert(channel.into()) {
                return Err(HostError::new("SessionAttach", "Duplicate channel"));
            }
            return Ok(ClientChannel {
                session: group.session.clone(),
                attach_token: token.into(),
                channel: channel.into(),
            });
        }
        if params.get("channel").is_some() {
            return Err(HostError::new("SessionAttach", "Token required"));
        }
        if self.groups.len() >= 16 {
            return Err(HostError::new(
                "ClientCapacity",
                "Sixteen GUI sessions are already connected",
            ));
        }
        let token = new_id()?;
        let session = Session::local(new_id()?, self.boot.clone())?;
        self.groups.insert(
            token.clone(),
            Group {
                session: session.clone(),
                peer: peer.map(str::to_owned),
                channels: BTreeSet::from(["control".into()]),
                active: true,
                closing: false,
            },
        );
        Ok(ClientChannel {
            session,
            attach_token: token,
            channel: "control".into(),
        })
    }
    pub fn revoke_peer(&mut self, peer: &str) -> Vec<Session> {
        self.groups.values_mut().filter_map(|g| {
            if g.active && g.peer.as_deref() == Some(peer) {
                g.active = false;
                Some(g.session.clone())
            } else { None }
        }).collect()
    }
    pub fn is_remote(&self, session: &str) -> bool {
        self.groups.values().any(|g| g.session.id() == session && g.peer.is_some())
    }
    pub(crate) fn peer_for(&self,session:&str)->Option<String> {
        self.groups.values().find(|g|g.session.id()==session).and_then(|g|g.peer.clone())
    }
    pub(crate) fn fence_release(&mut self,session:&str,peer:&str)->Result<(),HostError> {
        if let Some(group)=self.groups.values_mut().find(|g|g.session.id()==session) {
            if group.peer.as_deref()!=Some(peer) {return Err(HostError::new("ReleaseIdentity","Cannot fence another peer's session"));}
            group.closing=true;group.active=false;
        }Ok(())
    }
    pub fn active(&self, id: &str) -> bool {
        self.groups
            .values()
            .any(|g| g.session.id() == id && g.active)
    }
    pub fn begin_close(&mut self, id: &str) -> Result<(), HostError> {
        let group = self
            .groups
            .values_mut()
            .find(|g| g.session.id() == id)
            .ok_or_else(|| {
                HostError::new("ClientOffline", "Authenticated client is unavailable")
            })?;
        group.closing = true;
        Ok(())
    }
    pub fn control_allowed(&self, id: &str) -> bool {
        self.groups
            .values()
            .any(|g| g.session.id() == id && g.active && !g.closing)
    }
    pub fn leave(&mut self, client: &ClientChannel) -> Option<Session> {
        let group = self.groups.get_mut(&client.attach_token)?;
        if !group.channels.remove(&client.channel) {
            return None;
        }
        let revoke = group.active && !matches!(client.channel.as_str(), "results" | "status");
        if revoke {
            group.active = false;
        }
        let session = revoke.then(|| group.session.clone());
        if group.channels.is_empty() {
            self.groups.remove(&client.attach_token);
        }
        session
    }
}
