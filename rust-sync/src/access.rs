use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio_postgres::Transaction;

pub(crate) struct Grant {
    role: String,
    id: String,
    name: String,
    group: Option<String>,
    can_read: bool,
    can_write: bool,
    expires_at: Option<String>,
    invitation: Option<Value>,
    requires_setup: bool,
}

impl Grant {
    pub(crate) fn view(&self) -> Value {
        json!({ "role": self.role, "id": self.id(), "name": self.name(),
            "canRead": self.can_read(), "canWrite": self.can_write(), "expiresAt": self.expires_at() })
    }

    pub(crate) fn me(&self) -> Value {
        let mut account = self.view();
        if let Some(invitation) = &self.invitation {
            account["invitationProfile"] = invitation.clone();
        }
        if self.is_student() {
            json!({ "account": account })
        } else {
            json!({ "account": account, "teacher": self.view() })
        }
    }

    pub(crate) fn requires_setup(&self) -> bool {
        self.requires_setup
    }
    pub(crate) fn is_admin(&self) -> bool {
        self.role == "administrator"
    }
    pub(crate) fn role(&self) -> &str {
        &self.role
    }
    pub(crate) fn is_student(&self) -> bool {
        self.role == "student"
    }
    pub(crate) fn can_read(&self) -> bool {
        self.can_read
    }
    pub(crate) fn can_write(&self) -> bool {
        self.can_write
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn expires_at(&self) -> Option<&str> {
        self.expires_at.as_deref()
    }
    pub(crate) fn student_name_and_group(&self) -> Option<(&str, &str)> {
        self.group
            .as_deref()
            .map(|group| (self.name.as_str(), group))
    }
}

pub(crate) async fn load(
    tx: &Transaction<'_>,
    subject: &str,
) -> Result<Option<Grant>, tokio_postgres::Error> {
    let Some(account) = tx
        .query_opt(
            r#"SELECT role, teacher_id::text, student_id::text, active,
                  floor(extract(epoch from expires_at) * 1000)::bigint AS expires_ms,
                  invitation_username IS NOT NULL OR invitation_request_id IS NOT NULL AS invited,
                  verified_phone, invitation_phone, invitation_requires_setup,
                  to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS expiry
           FROM app_accounts WHERE auth_user_id = $1 FOR UPDATE"#,
            &[&subject],
        )
        .await?
    else {
        return Ok(None);
    };
    if !live(account.get("active"), account.get("expires_ms")) {
        return Ok(None);
    }
    let role: String = account.get("role");
    let invited: bool = account.get("invited");
    let setup: bool = account.get("invitation_requires_setup");
    let invitation = invited.then(|| {
        json!({
            "verifiedPhone": account.get::<_, Option<String>>("verified_phone"),
            "phone": account.get::<_, Option<String>>("invitation_phone"),
            "requiresSetup": setup,
        })
    });
    let (id, name, group, can_read, can_write, expires_at) = if role == "student" {
        let Some(id) = account.get::<_, Option<String>>("student_id") else {
            return Ok(None);
        };
        let Some(row) = tx.query_opt("SELECT id::text, name, group_name FROM students WHERE id = $1::text::uuid AND active FOR SHARE", &[&id]).await? else { return Ok(None) };
        (
            row.get("id"),
            row.get("name"),
            Some(row.get("group_name")),
            true,
            false,
            account.get("expiry"),
        )
    } else if role == "teacher" || role == "administrator" {
        let Some(id) = account.get::<_, Option<String>>("teacher_id") else {
            return Ok(None);
        };
        let Some(row) = tx
            .query_opt(
                r#"SELECT t.id::text, t.name, t.access_mode, t.active,
                      floor(extract(epoch from t.expires_at) * 1000)::bigint AS expires_ms,
                      to_char(LEAST(t.expires_at, a.expires_at) AT TIME ZONE 'UTC',
                        'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS expiry
               FROM teachers t JOIN app_accounts a ON a.auth_user_id = $2
               WHERE t.id = $1::text::uuid FOR UPDATE OF t"#,
                &[&id, &subject],
            )
            .await?
        else {
            return Ok(None);
        };
        if !live(row.get("active"), row.get("expires_ms")) {
            return Ok(None);
        }
        let mode: String = row.get("access_mode");
        (
            row.get("id"),
            row.get("name"),
            None,
            role == "teacher" || mode != "write_only",
            role == "teacher" || mode != "read_only",
            row.get("expiry"),
        )
    } else {
        return Ok(None);
    };
    Ok(Some(Grant {
        role,
        id,
        name,
        group,
        can_read,
        can_write,
        expires_at,
        invitation,
        requires_setup: invited && setup,
    }))
}

fn live(active: bool, expires_ms: Option<i64>) -> bool {
    active
        && expires_ms.is_none_or(|expires| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .is_ok_and(|now| {
                    u128::try_from(expires).is_ok_and(|expires| expires > now.as_millis())
                })
        })
}
