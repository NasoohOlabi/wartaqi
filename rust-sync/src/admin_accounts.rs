use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    aead, digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

use crate::{
    auth_browser::phone,
    credentials,
    ledger::{iso_datetime, js_trim, valid_uuid},
    staff_reads::admin_board,
    writes::Error,
};

const INVITE: &str = "/api/admin/accounts/invite";
const INVITE_MESSAGE: &str = "/api/admin/accounts/invite-message";
const VERIFY_PHONE: &str = "/api/admin/accounts/verify-phone";
const GRANT: &str = "/api/admin/accounts/grant";
const DISABLE: &str = "/api/admin/accounts/disable";
const RESET: &str = "/api/admin/accounts/reset-password";
const GENERATED_RESET: &str = "/api/admin/accounts/reset-password-generated";

pub(crate) const PATHS: &[&str] = &[
    INVITE,
    INVITE_MESSAGE,
    VERIFY_PHONE,
    GRANT,
    DISABLE,
    RESET,
    GENERATED_RESET,
];

#[derive(Clone, Copy)]
pub(crate) enum InvitationDelivery<'a> {
    Browser(&'a str),
    Mcp(&'a str),
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Administrator,
    Teacher,
    Student,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Administrator => "administrator",
            Role::Teacher => "teacher",
            Role::Student => "student",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InviteInput {
    request_id: String,
    people: Vec<Person>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Person {
    first_name: String,
    last_name: String,
    role: Role,
    phone: String,
    phone_confirmed: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Invitation {
    first_name: String,
    last_name: String,
    role: Role,
    phone: String,
    temporary_password: String,
}

#[derive(Deserialize, Serialize)]
struct Invitations {
    invitations: Vec<Invitation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevealInput {
    request_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct VerifyPhoneInput {
    auth_user_id: String,
    phone: String,
    phone_confirmed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyMode {
    ReadWrite,
    ReadOnly,
    WriteOnly,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Expiry {
    Date(String),
    None,
}

impl Expiry {
    fn date(&self) -> Result<Option<&str>, Error> {
        match self {
            Self::None => Ok(None),
            Self::Date(value) if iso_datetime(value) => Ok(Some(value)),
            Self::Date(_) => Err(Error::Invalid),
        }
    }
}

#[derive(Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "role",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
enum GrantInput {
    Administrator {
        auth_user_id: String,
        name: String,
    },
    Teacher {
        auth_user_id: String,
        name: String,
        expires_at: Expiry,
    },
    Student {
        auth_user_id: String,
        student_id: String,
        expires_at: Expiry,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AccountInput {
    auth_user_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PasswordInput {
    auth_user_id: String,
    password: String,
    password_confirmation: String,
}

fn input<T: DeserializeOwned>(body: Value) -> Result<T, Error> {
    serde_json::from_value(body).map_err(|_| Error::Invalid)
}

fn valid_name(value: &str, max: usize) -> Result<&str, Error> {
    let name = js_trim(value);
    if name.is_empty() || name.encode_utf16().count() > max {
        return Err(Error::Invalid);
    }
    Ok(name)
}

fn confirmed_phone(value: &str, confirmed: bool) -> Result<String, Error> {
    if !confirmed {
        return Err(Error::Invalid);
    }
    phone(value).ok_or(Error::Invalid)
}

fn other_account<'a>(id: &'a str, subject: &str, max: usize) -> Result<&'a str, Error> {
    if id.is_empty() || id.encode_utf16().count() > max {
        return Err(Error::Invalid);
    }
    if id == subject {
        return Err(Error::Forbidden);
    }
    Ok(id)
}

fn internal(message: &'static str) -> Error {
    Error::Other(message.into())
}

fn temporary_password() -> Result<String, Error> {
    let mut bytes = [0; 12];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| internal("Temporary password generation failed"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

async fn hash(password: &str) -> Result<String, Error> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || credentials::hash_password(&password))
        .await
        .map_err(|error| Error::Other(error.into()))?
        .map_err(internal)
}

pub(crate) async fn lock_auth(tx: &Transaction<'_>, path: &str) -> Result<(), Error> {
    if matches!(path, INVITE | VERIFY_PHONE | RESET | GENERATED_RESET) {
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended('auth-mutations', 0))",
            &[],
        )
        .await?;
    }
    Ok(())
}

async fn lock_id(tx: &Transaction<'_>, id: &str) -> Result<(), Error> {
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        &[&id],
    )
    .await?;
    Ok(())
}

async fn revoke_access(tx: &Transaction<'_>, subject: &str, sessions: bool) -> Result<(), Error> {
    if sessions {
        tx.execute(
            r#"DELETE FROM neon_auth.session WHERE "userId"::text = $1"#,
            &[&subject],
        )
        .await?;
    }
    tx.execute(
        "DELETE FROM neon_auth.verification
         WHERE starts_with(identifier, 'wartaq:telegram:') AND right(identifier, length($1)) = $1",
        &[&format!(":{subject}")],
    )
    .await?;
    Ok(())
}

async fn invite(
    tx: &Transaction<'_>,
    subject: &str,
    payload: InviteInput,
    delivery: InvitationDelivery<'_>,
) -> Result<Value, Error> {
    let request = &payload.request_id;
    if !valid_uuid(request) || !(1..=50).contains(&payload.people.len()) {
        return Err(Error::Invalid);
    }
    let people = payload
        .people
        .iter()
        .map(|person| {
            Ok((
                valid_name(&person.first_name, 49)?,
                valid_name(&person.last_name, 49)?,
                person.role,
                confirmed_phone(&person.phone, person.phone_confirmed)?,
            ))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    lock_id(tx, request).await?;
    if tx
        .query_opt(
            "SELECT 1 FROM app_accounts WHERE invitation_request_id = $1::text::uuid LIMIT 1",
            &[request],
        )
        .await?
        .is_some()
    {
        return Err(Error::Conflict);
    }
    let mut invitations = Vec::with_capacity(people.len());
    for (first, last, role, phone) in people {
        let password = temporary_password()?;
        let hash = hash(&password).await?;
        let name = format!("{first} {last}");
        let user: String = tx
            .query_one(
                r#"WITH identity AS (SELECT gen_random_uuid() AS id)
               INSERT INTO neon_auth."user" (id, name, email, "emailVerified")
               SELECT id, $1, id::text || '@invitation.invalid', true FROM identity
               RETURNING id::text"#,
                &[&name],
            )
            .await?
            .get(0);
        tx.execute(
            r#"INSERT INTO neon_auth.account (id, "accountId", "providerId", "userId", password)
               VALUES (gen_random_uuid(), $1, 'credential', $1::text::uuid, $2)"#,
            &[&user, &hash],
        )
        .await?;
        match role {
            Role::Student => tx.execute(
                "WITH profile AS (
                    INSERT INTO students (id, name, first_name, last_name, teacher_id)
                    VALUES (gen_random_uuid(), $1, $2, $3, NULL) RETURNING id
                 )
                 INSERT INTO app_accounts
                    (auth_user_id, role, student_id, invitation_request_id, verified_phone, invitation_requires_setup)
                 SELECT $4, 'student', id, $5::text::uuid, $6, true FROM profile",
                &[&name, &first, &last, &user, &request, &phone],
            ).await?,
            Role::Teacher | Role::Administrator => tx.execute(
                "WITH profile AS (
                    INSERT INTO teachers (id, auth_user_id, name, access_mode)
                    VALUES (gen_random_uuid(), $1, $2, 'read_write') RETURNING id
                 )
                 INSERT INTO app_accounts
                    (auth_user_id, role, teacher_id, invitation_request_id, verified_phone, invitation_requires_setup)
                 SELECT $1, $3, id, $4::text::uuid, $5, true FROM profile",
                &[&user, &name, &role.name(), &request, &phone],
            ).await?,
        };
        invitations.push(Invitation {
            first_name: first.to_owned(),
            last_name: last.to_owned(),
            role,
            phone,
            temporary_password: password,
        });
    }
    let invitations = Invitations { invitations };
    if let InvitationDelivery::Mcp(secret) = delivery {
        let sealed = seal(secret, subject, request, &invitations)?;
        tx.execute(
            r#"DELETE FROM neon_auth.verification
               WHERE identifier LIKE 'wartaq:invite-message:%' AND "expiresAt" <= now()"#,
            &[],
        )
        .await?;
        tx.execute(
            r#"INSERT INTO neon_auth.verification (id, identifier, value, "expiresAt")
               VALUES (gen_random_uuid(), $1, $2, now() + interval '15 minutes')"#,
            &[&receipt_id(subject, request), &sealed],
        )
        .await?;
        Ok(json!({"requestId": request}))
    } else {
        Ok(json!(invitations))
    }
}

fn receipt_id(subject: &str, request: &str) -> String {
    format!("wartaq:invite-message:{request}:{subject}")
}

fn receipt_key(secret: &str) -> Result<aead::LessSafeKey, Error> {
    let mut key = digest::Context::new(&digest::SHA256);
    key.update(b"wartaq:invite-message:v1\0");
    key.update(secret.as_bytes());
    aead::UnboundKey::new(&aead::AES_256_GCM, key.finish().as_ref())
        .map(aead::LessSafeKey::new)
        .map_err(|_| internal("Invalid invitation receipt key"))
}

fn receipt_aad(subject: &str, request: &str) -> aead::Aad<Vec<u8>> {
    aead::Aad::from(json!([subject, request]).to_string().into_bytes())
}

// The existing wire format stores nonce and tag before the ciphertext.
fn seal(
    secret: &str,
    subject: &str,
    request: &str,
    invitations: &Invitations,
) -> Result<String, Error> {
    let mut nonce = [0; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| internal("Invitation receipt nonce generation failed"))?;
    let mut bytes = serde_json::to_vec(invitations)
        .map_err(|_| internal("Invitation receipt serialization failed"))?;
    let tag = receipt_key(secret)?
        .seal_in_place_separate_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            receipt_aad(subject, request),
            &mut bytes,
        )
        .map_err(|_| internal("Invitation receipt encryption failed"))?;
    Ok(URL_SAFE_NO_PAD.encode([nonce.as_slice(), tag.as_ref(), &bytes].concat()))
}

fn open(secret: &str, subject: &str, request: &str, encoded: &str) -> Result<Value, Error> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| internal("Invalid invitation receipt encoding"))?;
    let Some((prefix, ciphertext)) = bytes.split_at_checked(28) else {
        return Err(internal("Invalid invitation receipt length"));
    };
    let (nonce, tag) = prefix.split_at(12);
    let nonce = aead::Nonce::try_assume_unique_for_key(nonce)
        .map_err(|_| internal("Invalid invitation receipt nonce"))?;
    let mut bytes = [ciphertext, tag].concat();
    let plain = receipt_key(secret)?
        .open_in_place(nonce, receipt_aad(subject, request), &mut bytes)
        .map_err(|_| internal("Invitation receipt authentication failed"))?;
    let invitations: Invitations = serde_json::from_slice(plain)
        .map_err(|_| internal("Invalid invitation receipt content"))?;
    Ok(json!(invitations))
}

async fn reveal(
    tx: &Transaction<'_>,
    subject: &str,
    body: RevealInput,
    secret: &str,
) -> Result<Value, Error> {
    if !valid_uuid(&body.request_id) {
        return Err(Error::Invalid);
    }
    let row = tx
        .query_opt(
            r#"DELETE FROM neon_auth.verification
           WHERE identifier = $1 AND "expiresAt" > now() RETURNING value"#,
            &[&receipt_id(subject, &body.request_id)],
        )
        .await?
        .ok_or(Error::NotFound)?;
    open(secret, subject, &body.request_id, row.get(0))
}

async fn verify_phone(
    tx: &Transaction<'_>,
    subject: &str,
    data: VerifyPhoneInput,
) -> Result<(), Error> {
    let phone = confirmed_phone(&data.phone, data.phone_confirmed)?;
    let target = other_account(&data.auth_user_id, subject, usize::MAX)?;
    let Some(row) = tx
        .query_opt(
            "SELECT verified_phone FROM app_accounts WHERE auth_user_id = $1 FOR UPDATE",
            &[&target],
        )
        .await?
    else {
        return Err(Error::Invalid);
    };
    let changed = tx.execute(
        "UPDATE app_accounts a SET verified_phone = $2
         WHERE auth_user_id = $1 AND active AND (expires_at IS NULL OR expires_at > now())
         AND ((role = 'student' AND EXISTS (SELECT 1 FROM students s WHERE s.id = a.student_id AND s.active))
           OR (role IN ('teacher', 'administrator') AND EXISTS (
               SELECT 1 FROM teachers t WHERE t.id = a.teacher_id AND t.active
               AND (t.expires_at IS NULL OR t.expires_at > now()))))",
        &[&target, &phone],
    ).await?;
    if changed == 0 {
        return Err(Error::Invalid);
    }
    if row.get::<_, Option<&str>>(0) != Some(phone.as_str()) {
        revoke_access(tx, target, true).await?;
    }
    Ok(())
}

enum GrantTarget<'a> {
    Student(&'a str),
    Staff { role: Role, name: &'a str },
}

async fn grant(tx: &Transaction<'_>, subject: &str, mut body: Value) -> Result<(), Error> {
    if let Some(mode) = body
        .as_object_mut()
        .and_then(|object| object.remove("mode"))
    {
        input::<LegacyMode>(mode)?;
    }
    let grant: GrantInput = input(body)?;
    let (user, target, expiry) = match &grant {
        GrantInput::Administrator { auth_user_id, name } => (
            auth_user_id,
            GrantTarget::Staff {
                role: Role::Administrator,
                name: valid_name(name, 100)?,
            },
            None,
        ),
        GrantInput::Teacher {
            auth_user_id,
            name,
            expires_at,
        } => (
            auth_user_id,
            GrantTarget::Staff {
                role: Role::Teacher,
                name: valid_name(name, 100)?,
            },
            expires_at.date()?,
        ),
        GrantInput::Student {
            auth_user_id,
            student_id,
            expires_at,
        } => {
            if !valid_uuid(student_id) {
                return Err(Error::Invalid);
            }
            (
                auth_user_id,
                GrantTarget::Student(student_id),
                expires_at.date()?,
            )
        }
    };
    let user = other_account(user, subject, 256)?;
    lock_id(tx, user).await?;
    if tx
        .query_opt(
            r#"SELECT id FROM neon_auth."user" WHERE id::text = $1 FOR SHARE"#,
            &[&user],
        )
        .await?
        .is_none()
    {
        return Err(Error::Invalid);
    }
    tx.query_opt(
        "SELECT auth_user_id FROM app_accounts WHERE auth_user_id = $1 FOR UPDATE",
        &[&user],
    )
    .await?;
    let (role, teacher, student) = match target {
        GrantTarget::Student(student) => {
            if tx
                .query_opt(
                    "SELECT id FROM students WHERE id = $1::text::uuid FOR SHARE",
                    &[&student],
                )
                .await?
                .is_none()
            {
                return Err(Error::Invalid);
            }
            (Role::Student, None, Some(student))
        }
        GrantTarget::Staff { role, name } => {
            let teacher: String = tx.query_one(
                "INSERT INTO teachers (id, auth_user_id, name, active, access_mode, expires_at)
                 VALUES (gen_random_uuid(), $1, $2, true, 'read_write', $3::text::timestamptz)
                 ON CONFLICT (auth_user_id) DO UPDATE SET name = EXCLUDED.name, active = true,
                    access_mode = EXCLUDED.access_mode, expires_at = EXCLUDED.expires_at RETURNING id::text",
                &[&user, &name, &expiry],
            ).await?.get(0);
            (role, Some(teacher), None)
        }
    };
    tx.execute(
        "INSERT INTO app_accounts (auth_user_id, role, teacher_id, student_id, active, expires_at)
         VALUES ($1, $2, $3::text::uuid, $4::text::uuid, true, $5::text::timestamptz)
         ON CONFLICT (auth_user_id) DO UPDATE SET role = EXCLUDED.role, teacher_id = EXCLUDED.teacher_id,
            student_id = EXCLUDED.student_id, active = true, expires_at = EXCLUDED.expires_at",
        &[&user, &role.name(), &teacher, &student, &expiry],
    ).await?;
    revoke_access(tx, user, false).await
}

async fn reset(
    tx: &Transaction<'_>,
    target: &str,
    password: &str,
    generated: bool,
) -> Result<Option<Value>, Error> {
    let row = tx
        .query_opt(
            "SELECT verified_phone FROM app_accounts WHERE auth_user_id = $1 FOR UPDATE",
            &[&target],
        )
        .await?;
    let mut result = None;
    if generated {
        let phone = row
            .as_ref()
            .and_then(|row| row.get::<_, Option<&str>>(0))
            .filter(|phone| !phone.is_empty())
            .ok_or(Error::Invalid)?;
        result = Some(
            json!({"loginIdentifier": phone, "loginMethod": "phone", "temporaryPassword": password}),
        );
    }
    let hash = hash(password).await?;
    let changed = tx
        .execute(
            r#"UPDATE neon_auth.account SET password = $2, "updatedAt" = now()
           WHERE "userId"::text = $1 AND "providerId" = 'credential'"#,
            &[&target, &hash],
        )
        .await?;
    if changed != 1 {
        return Err(Error::Invalid);
    }
    revoke_access(tx, target, true).await?;
    Ok(result)
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    path: &str,
    subject: &str,
    body: Value,
    delivery: InvitationDelivery<'_>,
) -> Result<Value, Error> {
    match path {
        INVITE => return invite(tx, subject, input(body)?, delivery).await,
        INVITE_MESSAGE => {
            let InvitationDelivery::Browser(secret) = delivery else {
                return Err(Error::NotFound);
            };
            return reveal(tx, subject, input(body)?, secret).await;
        }
        VERIFY_PHONE => verify_phone(tx, subject, input(body)?).await?,
        GRANT => grant(tx, subject, body).await?,
        DISABLE => {
            let data: AccountInput = input(body)?;
            let target = other_account(&data.auth_user_id, subject, 256)?;
            lock_id(tx, target).await?;
            tx.execute(
                "UPDATE app_accounts SET active = false WHERE auth_user_id = $1",
                &[&target],
            )
            .await?;
            revoke_access(tx, target, false).await?;
        }
        GENERATED_RESET => {
            if !matches!(delivery, InvitationDelivery::Browser(_)) {
                return Err(Error::Forbidden);
            }
            let data: AccountInput = input(body)?;
            let target = other_account(&data.auth_user_id, subject, 256)?;
            let password = temporary_password()?;
            return reset(tx, target, &password, true)
                .await?
                .ok_or(Error::Invalid);
        }
        RESET => {
            let data: PasswordInput = input(body)?;
            if data.password != data.password_confirmation
                || !(8..=128).contains(&data.password.encode_utf16().count())
            {
                return Err(Error::Invalid);
            }
            let target = other_account(&data.auth_user_id, subject, 256)?;
            reset(tx, target, &data.password, false).await?;
        }
        _ => return Err(Error::NotFound),
    }
    admin_board(tx).await.map_err(Error::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_existing_node_receipts_for_the_creating_admin_only() {
        let secret = "fixture-only-secret";
        let subject = "e0cf5e35-39e8-4929-9408-fb5bc9688b70";
        let request = "60cf5e35-39e8-4929-9408-fb5bc9688b71";
        // Synthetic receipt generated by Node's AES-256-GCM implementation.
        let stored = concat!(
            "AAECAwQFBgcICQoLKjfl6uRiT8uyV9O-r5gqtfGl3CFAxcwxY38OiTYGNX8lIC2ecLm17VpB041-C_5NmI4BXM_UXh_DIZWJ",
            "kBbTYpft9KPM3VTYWJ7AVCm0hxEdNTzjTlQqVmI3lDL7MloiuIyZHNQgtR6iT9P4uFZ9Dv2wTnsyoRLH8WzGCw5mDqHq7tX",
            "yq1J1AgAE8DFBgtd91zzEPRzCoGReW8XShfYS"
        );
        let decoded = open(secret, subject, request, stored).unwrap();
        assert_eq!(
            decoded,
            json!({"invitations":[{
                "firstName": "Synthetic",
                "lastName": "Teacher",
                "role": "teacher",
                "phone": "+12025550109",
                "temporaryPassword": "fixture-password"
            }]})
        );
        assert!(open(secret, request, subject, stored).is_err());
        let invitations = input::<Invitations>(decoded.clone()).unwrap();
        let encoded = seal(secret, subject, request, &invitations).unwrap();
        assert_eq!(open(secret, subject, request, &encoded).unwrap(), decoded);
    }

    #[test]
    fn grant_expiry_is_required_and_nullable_for_teachers_and_students() {
        let missing = json!({"role":"teacher", "authUserId":"target", "name":"Synthetic"});
        let mut with_expiry = missing.clone();
        with_expiry["expiresAt"] = Value::Null;
        assert!(input::<GrantInput>(missing).is_err());
        assert!(matches!(
            input::<GrantInput>(with_expiry),
            Ok(GrantInput::Teacher {
                expires_at: Expiry::None,
                ..
            })
        ));
        assert!(
            input::<GrantInput>(json!({
                "role":"administrator", "authUserId":"target", "name":"Synthetic", "expiresAt":null
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn mcp_invite_creates_an_encrypted_one_use_browser_receipt() {
        let Ok(database_url) = std::env::var("TEST_RAILWAY_DATABASE_URL") else {
            return;
        };
        let (mut db, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move { connection.await.unwrap() });
        let tx = db.transaction().await.unwrap();
        let row = tx
            .query_one(
                "SELECT gen_random_uuid()::text, gen_random_uuid()::text",
                &[],
            )
            .await
            .unwrap();
        let subject: &str = row.get(0);
        let request: &str = row.get(1);
        let mut random = [0; 4];
        SystemRandom::new().fill(&mut random).unwrap();
        let phone = format!("+199{:09}", u32::from_be_bytes(random) % 1_000_000_000);
        let secret = "fixture-only-secret";
        lock_auth(&tx, INVITE).await.unwrap();
        let link = apply(
            &tx,
            INVITE,
            subject,
            json!({
                "requestId": request,
                "people": [{
                    "firstName": "Synthetic", "lastName": "Student", "role": "student",
                    "phone": phone, "phoneConfirmed": true
                }]
            }),
            InvitationDelivery::Mcp(secret),
        )
        .await
        .unwrap();
        let record = tx
            .query_one(
                r#"SELECT c.password, v.value
               FROM app_accounts a
               JOIN neon_auth.account c ON c."userId"::text = a.auth_user_id
               JOIN neon_auth.verification v ON v.identifier = $2 AND v."expiresAt" > now()
               WHERE a.invitation_request_id = $1::text::uuid AND a.invitation_requires_setup"#,
                &[&request, &receipt_id(subject, request)],
            )
            .await
            .unwrap();
        let cipher: &str = record.get(1);
        assert_eq!(link, json!({"requestId": request}));
        assert!(!cipher.contains(&phone));
        assert!(matches!(
            apply(
                &tx,
                INVITE_MESSAGE,
                request,
                link.clone(),
                InvitationDelivery::Browser(secret),
            )
            .await,
            Err(Error::NotFound)
        ));
        let message = apply(
            &tx,
            INVITE_MESSAGE,
            subject,
            link.clone(),
            InvitationDelivery::Browser(secret),
        )
        .await
        .unwrap();
        assert_eq!(message["invitations"][0]["phone"], phone);
        let password = message["invitations"][0]["temporaryPassword"]
            .as_str()
            .unwrap();
        assert!(!cipher.contains(password));
        assert!(credentials::verify_password(password, record.get(0)));
        assert!(matches!(
            apply(
                &tx,
                INVITE_MESSAGE,
                subject,
                link,
                InvitationDelivery::Browser(secret),
            )
            .await,
            Err(Error::NotFound)
        ));
        tx.rollback().await.unwrap();
    }
}
