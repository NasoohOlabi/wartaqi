use deadpool_postgres::Pool;
use serde_json::{Value, json};
use tokio_postgres::Transaction;

use crate::{
    access::{self, Grant},
    admin_accounts, auth_invitation_profile, auth_profile, catalog, catalog_legacy, ledger,
    ledger_writes,
    mcp_tools::{Access, EditVersion, Operation, ResponseMapping, Role, map_failure, map_success},
    oauth, roster_admin, staff_reads, student_writes, telegram_teacher,
    writes::Error,
};

#[derive(Clone, Copy)]
pub(crate) enum Identity<'a> {
    Telegram(&'a telegram_teacher::Credential),
    OAuth(&'a oauth::Grant),
}

impl Identity<'_> {
    fn subject(&self) -> &str {
        match self {
            Self::Telegram(credential) => credential.subject(),
            Self::OAuth(grant) => &grant.user_id,
        }
    }

    fn telegram(&self) -> bool {
        matches!(self, Self::Telegram(_))
    }

    async fn authorize(&self, tx: &Transaction<'_>) -> Result<Option<Grant>, Error> {
        match self {
            Self::Telegram(credential) => Ok(telegram_teacher::authorize(tx, credential).await?),
            Self::OAuth(credential) => {
                if !credential.still_active(tx).await? {
                    return Ok(None);
                }
                Ok(access::load(tx, self.subject()).await?)
            }
        }
    }

    fn has_scope(&self, wanted: &str) -> bool {
        match self {
            Self::Telegram(_) => true,
            Self::OAuth(grant) => grant.scopes.iter().any(|scope| scope == wanted),
        }
    }
}

pub(crate) struct Session {
    pub access: Access,
    pub year: i32,
}

pub(crate) async fn open(pool: &Pool, identity: Identity<'_>) -> Result<Option<Session>, Error> {
    let mut db = pool
        .get()
        .await
        .map_err(|error| Error::Other(error.into()))?;
    let tx = db.transaction().await?;
    let Some(grant) = identity.authorize(&tx).await? else {
        return Ok(None);
    };
    let year = tx
        .query_one("SELECT extract(year from now())::int", &[])
        .await?
        .get(0);
    tx.commit().await?;
    let role = if grant.is_admin() {
        Role::Administrator
    } else if grant.is_student() {
        Role::Student
    } else {
        Role::Teacher
    };
    Ok(Some(Session {
        access: Access {
            role,
            read_scope: identity.has_scope("mcp:read"),
            write_scope: identity.has_scope("mcp:write"),
            can_read: grant.can_read(),
            can_write: grant.can_write(),
            ready: identity.telegram() || !grant.requires_setup(),
            invitation: !identity.telegram()
                && grant.me()["account"].get("invitationProfile").is_some(),
            user_id: identity.subject().to_owned(),
            name: grant.name().to_owned(),
        },
        year,
    }))
}

pub(crate) fn text(message: &str, is_error: bool) -> Value {
    let mut result = json!({"content":[{"type":"text","text":message}]});
    if is_error {
        result["isError"] = json!(true);
    }
    result
}

pub(crate) async fn call(
    pool: &Pool,
    origin: &str,
    secret: Option<&str>,
    identity: Identity<'_>,
    operation: Operation,
) -> Value {
    let result = execute(pool, origin, secret, identity, &operation).await;
    match result {
        Ok(value) => {
            let result = match &operation.response {
                ResponseMapping::Invitation => return invitation_result(value, origin),
                ResponseMapping::Handoff => return text(value.as_str().unwrap_or_default(), false),
                ResponseMapping::CourseExport(filter) => crate::mcp_courses::export(&value, filter),
                mapping => {
                    map_success(mapping, value, origin).map(|value| text(&value.to_string(), false))
                }
            };
            result.unwrap_or_else(|message| text(message, true))
        }
        Err(problem) => {
            let message = match problem {
                Error::Invalid if operation.path == "/api/mcp/profile" => "Invalid account data",
                Error::Invalid => "Invalid request",
                Error::Forbidden => "Forbidden",
                Error::Conflict => "Conflict",
                Error::TooLarge => "Request too large",
                Error::Other(problem) if problem.is::<ledger::InvalidData>() => "Invalid request",
                Error::NotFound | Error::Other(_) => "Server error",
            };
            text(
                &map_failure(&operation.response, &json!({"error":message}).to_string()),
                true,
            )
        }
    }
}

fn link(origin: &str, options: &[(&str, &str)]) -> Result<String, Error> {
    let mut url = url::Url::parse(origin).map_err(|error| Error::Other(error.into()))?;
    url.query_pairs_mut()
        .append_pair("page", "app")
        .extend_pairs(options.iter().copied());
    Ok(url.into())
}

fn invitation_result(value: Value, origin: &str) -> Value {
    let Some(request_id) = value["requestId"]
        .as_str()
        .filter(|id| ledger::valid_uuid(id))
    else {
        return text("Invitation response invalid", true);
    };
    let Ok(url) = link(
        origin,
        &[
            ("view", "invitation-message"),
            ("inviteMessage", request_id),
        ],
    ) else {
        return text("Invitation link unavailable", true);
    };
    let mut result = text(&format!("Open the private invitation: {url}"), false);
    result["structuredContent"] = json!({"requestId":request_id,"url":url});
    result
}

async fn execute(
    pool: &Pool,
    origin: &str,
    secret: Option<&str>,
    identity: Identity<'_>,
    operation: &Operation,
) -> Result<Value, Error> {
    let mut db = pool
        .get()
        .await
        .map_err(|error| Error::Other(error.into()))?;
    let tx = db.transaction().await?;
    let subject = identity.subject();
    let path = operation.path;
    if path == "/api/me/invitation-profile" {
        tx.execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('auth-mutations', 0))",
            &[],
        )
        .await?;
    }
    if admin_accounts::PATHS.contains(&path) {
        admin_accounts::lock_auth(&tx, path).await?;
    }
    let grant = identity.authorize(&tx).await?.ok_or(Error::Forbidden)?;
    let profile_pending = grant.requires_setup() && !identity.telegram();
    let staff = !grant.is_student() && !profile_pending;
    let admin = staff && grant.is_admin() && grant.can_read() && grant.can_write();
    let body = &operation.body;
    let input = || body.as_ref().ok_or(Error::Invalid);
    if let Some(edit) = &operation.edit {
        apply_edit_version(&tx, edit).await?;
    }
    let result = match (operation.method, path) {
        ("GET", "/api/me") if identity.telegram() => {
            json!({"account":grant.view(), "teacher":grant.view()})
        }
        ("GET", "/api/me") => grant.me(),
        ("GET" | "POST", "/api/mcp/profile") => profile(&tx, subject, body.as_ref()).await?,
        ("POST", "/api/me/invitation-profile") if !profile_pending && !identity.telegram() => {
            auth_invitation_profile::save_with_access(&tx, subject, None, input().cloned()).await?
        }
        ("GET", "/mcp/handoff/setup") if profile_pending => {
            json!(format!(
                "Open this link and sign in as the account being set up: {}",
                link(origin, &[])?
            ))
        }
        ("GET", "/mcp/handoff/profile")
            if !profile_pending
                && !identity.telegram()
                && grant.me()["account"].get("invitationProfile").is_some() =>
        {
            json!(format!(
                "Open your profile and sign in as the intended account: {}",
                link(origin, &[("view", "profile")])?
            ))
        }
        ("GET", "/mcp/handoff/reset") if admin => {
            let mut options = vec![("view", "settings")];
            if let Some(id) = input()?["authUserId"].as_str() {
                options.push(("account", id));
            }
            json!(format!(
                "Open Accounts as the intended administrator, then select Reset password. {}",
                link(origin, &options)?
            ))
        }
        ("GET", "/api/notebook") if staff && grant.can_read() => {
            staff_reads::snapshot(&tx, &grant, true)
                .await
                .map_err(Error::Other)?
        }
        ("GET", "/api/student/attendance") if !profile_pending && grant.is_student() => {
            let (name, group) = grant.student_name_and_group().ok_or(Error::Forbidden)?;
            ledger::student_attendance(&tx, grant.id(), name, group)
                .await
                .map_err(Error::Other)?
        }
        ("GET", "/api/admin") if admin => {
            staff_reads::admin_board(&tx).await.map_err(Error::Other)?
        }
        ("GET", "/mcp/course-workflow" | "/mcp/course-report") if admin => {
            let snapshot = staff_reads::snapshot(&tx, &grant, true)
                .await
                .map_err(Error::Other)?;
            let board = staff_reads::admin_board(&tx).await.map_err(Error::Other)?;
            json!({"notebook":snapshot,"admin":board})
        }
        ("POST", "/api/student/update") if staff && grant.can_write() => {
            student_writes::apply(&tx, path, subject, &grant, input()?.clone()).await?
        }
        ("POST", "/api/student/sync") if staff && grant.can_read() && grant.can_write() => {
            sync_student(&tx, subject, &grant, input()?).await?
        }
        ("POST", "/api/attendance/sync") if staff && grant.can_read() && grant.can_write() => {
            ledger_writes::apply(&tx, path, subject, &grant, input()?.clone()).await?
        }
        ("POST", "/api/events") if staff && grant.can_write() => {
            ledger_writes::apply(&tx, path, subject, &grant, input()?.clone()).await?
        }
        ("POST", path) if admin && admin_accounts::PATHS.contains(&path) => {
            let secret = secret.ok_or(Error::Forbidden)?;
            admin_accounts::apply(
                &tx,
                path,
                subject,
                input()?.clone(),
                admin_accounts::InvitationDelivery::Mcp(secret),
            )
            .await?
        }
        ("POST", "/api/admin/students" | "/api/admin/students/enroll") if admin => {
            roster_admin::apply(&tx, path, input()?.clone()).await?
        }
        ("POST", "/api/admin/enrollments/decision" | "/api/admin/courses/status") if admin => {
            catalog_legacy::apply(&tx, path, subject, input()?.clone()).await?
        }
        ("POST", path) if admin && path.starts_with("/api/admin/") => {
            catalog::apply(&tx, path, subject, input()?.clone()).await?
        }
        _ => return Err(Error::Forbidden),
    };
    tx.commit().await?;
    Ok(result)
}

async fn apply_edit_version(tx: &Transaction<'_>, edit: &EditVersion) -> Result<(), Error> {
    if tx
        .query_one(
            "SELECT $1::text::timestamptz > clock_timestamp() + interval '5 minutes'",
            &[&edit.time],
        )
        .await?
        .get::<_, bool>(0)
    {
        return Err(Error::Conflict);
    }
    tx.query_one(
        "SELECT set_config('wartaq.edit_time', $1, true), set_config('wartaq.edit_id', $2, true)",
        &[&edit.time, &edit.id],
    )
    .await?;
    Ok(())
}

async fn profile(
    tx: &Transaction<'_>,
    subject: &str,
    input: Option<&Value>,
) -> Result<Value, Error> {
    let row = if let Some(input) = input {
        if !auth_profile::profile_valid(input) {
            return Err(Error::Invalid);
        }
        let name = input["name"]
            .as_str()
            .map(ledger::js_trim)
            .ok_or(Error::Invalid)?;
        let image = input["image"].as_str();
        tx.query_one("UPDATE neon_auth.\"user\" SET name = $2, image = CASE WHEN $3 THEN $4::text ELSE image END, \"updatedAt\" = now() WHERE id = $1::text::uuid RETURNING name, image", &[&subject, &name, &input.get("image").is_some(), &image]).await?
    } else {
        tx.query_one(
            "SELECT name, image FROM neon_auth.\"user\" WHERE id = $1::text::uuid",
            &[&subject],
        )
        .await?
    };
    Ok(json!({"name":row.get::<_, String>(0), "image":row.get::<_, Option<String>>(1)}))
}

async fn sync_student(
    tx: &Transaction<'_>,
    subject: &str,
    grant: &Grant,
    input: &Value,
) -> Result<Value, Error> {
    match input["operation"]["kind"].as_str() {
        Some("note") => student_writes::sync_note(tx, subject, grant, input).await?,
        Some("profile-v1") => student_writes::sync_profile(tx, subject, grant, input).await?,
        _ => None,
    }
    .ok_or(Error::Invalid)
}
