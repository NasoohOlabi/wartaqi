use chrono::Datelike;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::worker::normalize_phone;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Administrator,
    Teacher,
    Student,
}

impl Role {
    fn wire(self) -> &'static str {
        match self {
            Self::Administrator => "administrator",
            Self::Teacher => "teacher",
            Self::Student => "student",
        }
    }

    fn label(self, lang: Language) -> &'static str {
        match self {
            Self::Administrator => lang.text("Administrator", "مدير"),
            Self::Teacher => lang.text("Teacher", "معلم"),
            Self::Student => lang.text("Student", "طالب"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedAccount {
    auth_user_id: String,
    name: String,
    profile_name: Option<String>,
    verified_phone: Option<String>,
    role: Option<Role>,
    active: bool,
    expires_at: Option<chrono::DateTime<Utc>>,
}

impl ListedAccount {
    fn name(&self) -> &str {
        self.profile_name
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.name)
    }
}

#[derive(Deserialize)]
struct AccountBoard {
    accounts: Vec<ListedAccount>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvitationRequest {
    request_id: String,
    people: [Invitee; 1],
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Invitee {
    first_name: String,
    last_name: String,
    role: Role,
    phone: String,
    phone_confirmed: bool,
}

impl Invitee {
    fn valid(&self) -> bool {
        valid_invitee_name(&self.first_name)
            && valid_invitee_name(&self.last_name)
            && self.phone_confirmed
            && normalize_phone(&self.phone, false).as_deref() == Some(&self.phone)
    }
}

fn valid_invitee_name(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && value.encode_utf16().count() <= 49
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InvitationResult {
    request_id: String,
    url: Url,
}

pub(super) async fn accounts(
    mcp: &Mcp<'_>,
    origin: &Url,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
) -> Result<Screen> {
    if !tools.contains("get_admin_board") {
        return Ok(unavailable(lang));
    }
    let list = Action::new("ac.list");
    let invite = Action::new("ac.invite");
    let t = |en: &'static str, ar: &'static str| lang.text(en, ar);
    let back_rows = |action| vec![vec![back(lang, action), home(lang)]];
    let prompt = |text, action, back| Screen::new(text, back_rows(back)).prompt(action);
    match action.route.as_str() {
        "ac.list" | "ac.search" | "ac.view" => {
            if action.route == "ac.search" && input.is_none() {
                return Ok(prompt(
                    t(
                        "Send a name to search, or /cancel.",
                        "أرسل اسماً للبحث، أو /cancel للإلغاء.",
                    ),
                    action.clone(),
                    list,
                ));
            }
            let board: AccountBoard =
                serde_json::from_value(mcp.call("get_admin_board", json!({})).await?)?;
            if action.route == "ac.view" {
                let Some(account) = board
                    .accounts
                    .iter()
                    .find(|account| account.auth_user_id == action.value("account"))
                else {
                    return Ok(unavailable(lang));
                };
                let role = account.role.map_or(
                    t("No approved role", "لا يوجد دور معتمد"),
                    |role| role.label(lang),
                );
                let status = if !account.active {
                    t("Disabled", "معطل")
                } else if account
                    .expires_at
                    .is_some_and(|expires| expires <= Utc::now())
                {
                    t("Expired", "منتهي الصلاحية")
                } else {
                    t("Active", "نشط")
                };
                let expires = account
                    .expires_at
                    .map(|date| date.to_rfc3339())
                    .unwrap_or_else(|| t("None", "لا يوجد").into());
                let mut link = origin.join("/")?;
                link.query_pairs_mut().extend_pairs([
                    ("page", "app"),
                    ("view", "settings"),
                    ("settingsSection", "accounts"),
                    ("account", &account.auth_user_id),
                ]);
                return Ok(Screen::new(
                    format!(
                        "{}\n{}: {role}\n{}: {status}\n{}: {expires}\n{}: {}",
                        short(account.name(), 120),
                        t("Role", "الدور"),
                        t("Status", "الحالة"),
                        t("Expiry", "انتهاء الصلاحية"),
                        t("Phone", "الهاتف"),
                        account.verified_phone.as_deref().unwrap_or("—")
                    ),
                    vec![
                        vec![Button::Link(
                            t("Manage account securely", "إدارة الحساب بأمان").into(),
                            link,
                        )],
                        vec![back(lang, list), home(lang)],
                    ],
                ));
            }
            let query = input
                .unwrap_or_else(|| action.value("q"))
                .trim()
                .chars()
                .take(200)
                .collect::<String>();
            let matches = rank_by_name(&board.accounts, &query, |account| account.name());
            let index = page(action);
            let mut rows: Vec<_> = matches
                .iter()
                .skip(index * PAGE)
                .take(PAGE)
                .map(|account| {
                    vec![choice(
                        account.name(),
                        Action::new("ac.view").arg("account", &account.auth_user_id),
                    )]
                })
                .collect();
            rows.extend(nav(lang, list.arg("q", query), index, matches.len()));
            rows.push(vec![choice(
                t("Search accounts", "بحث عن حساب"),
                Action::new("ac.search"),
            )]);
            if tools.contains("invite_accounts") {
                rows.push(vec![choice(t("Invite account", "دعوة حساب"), invite)]);
            }
            rows.push(vec![home(lang)]);
            return Ok(Screen::new(t("Accounts", "الحسابات"), rows));
        }
        _ if !tools.contains("invite_accounts") => return Ok(unavailable(lang)),
        "ac.invite" => {
            let button = |role: Role| {
                choice(
                    role.label(lang),
                    Action::new("ac.invite.name").arg("role", role.wire()),
                )
            };
            return Ok(Screen::new(
                t(
                    "Choose a role for the invitation.",
                    "اختر دور الحساب المدعو.",
                ),
                vec![
                    vec![button(Role::Administrator), button(Role::Teacher)],
                    vec![button(Role::Student)],
                    vec![back(lang, list), home(lang)],
                ],
            ));
        }
        _ => {}
    }
    let role: Role = match serde_json::from_value(json!(action.value("role"))) {
        Ok(role) => role,
        Err(_) => return Ok(unavailable(lang)),
    };
    let phone_prompt = t(
        "Send their phone number with country code, or /cancel.",
        "أرسل رقم هاتفه مع رمز البلد، أو /cancel للإلغاء.",
    );
    match action.route.as_str() {
        "ac.invite.name" => {
            let Some(text) = input else {
                return Ok(prompt(
                    t(
                        "Send first and last name separated by a comma, or /cancel.",
                        "أرسل الاسم الأول واسم العائلة مفصولين بفاصلة، أو /cancel للإلغاء.",
                    ),
                    action.clone(),
                    invite,
                ));
            };
            let parts: Vec<_> = text.split([',', '،']).map(str::trim).collect();
            let [first, last] = parts.as_slice() else {
                return Ok(prompt(
                    t(
                        "Enter first and last name separated by one comma.",
                        "أدخل الاسم الأول واسم العائلة بينهما فاصلة واحدة.",
                    ),
                    action.clone(),
                    invite,
                ));
            };
            if !valid_invitee_name(first) || !valid_invitee_name(last) {
                return Ok(prompt(
                    t(
                        "Enter first and last names of 1–49 characters each.",
                        "أدخل الاسم الأول واسم العائلة، من ١ إلى ٤٩ حرفاً لكل منهما.",
                    ),
                    action.clone(),
                    invite,
                ));
            }
            let next = Action::new("ac.invite.phone")
                .arg("role", role.wire())
                .arg("firstName", *first)
                .arg("lastName", *last);
            Ok(prompt(phone_prompt, next, invite))
        }
        "ac.invite.phone" => {
            let Some(text) = input else {
                return Ok(prompt(phone_prompt, action.clone(), invite));
            };
            let Some(phone) = normalize_phone(text, false) else {
                return Ok(prompt(
                    t(
                        "Enter a valid international phone number.",
                        "أدخل رقم هاتف دولياً صحيحاً.",
                    ),
                    action.clone(),
                    invite,
                ));
            };
            let person = Invitee {
                first_name: action.value("firstName").into(),
                last_name: action.value("lastName").into(),
                role,
                phone,
                phone_confirmed: true,
            };
            if !person.valid() {
                return Ok(unavailable(lang));
            }
            let text = format!(
                "{}\n{} {}\n{}\n{}",
                t(
                    "Have you asked this person directly and confirmed their phone number?",
                    "هل سألت هذا الشخص مباشرة وأكدت رقم هاتفه؟"
                ),
                person.first_name,
                person.last_name,
                person.phone,
                person.role.label(lang)
            );
            let payload = InvitationRequest {
                request_id: uuid()?,
                people: [person],
            };
            Ok(Screen::new(
                text,
                vec![
                    vec![choice(
                        t("Yes, confirmed directly", "نعم، أكدته مباشرة"),
                        Action::new("ac.invite.send")
                            .arg("role", role.wire())
                            .arg("payload", serde_json::to_string(&payload)?),
                    )],
                    vec![back(lang, invite), home(lang)],
                ],
            ))
        }
        "ac.invite.send" => {
            let payload: InvitationRequest = serde_json::from_str(action.value("payload"))?;
            if !id_shape(&payload.request_id) || !payload.people[0].valid() {
                return Ok(unavailable(lang));
            }
            let result: InvitationResult = serde_json::from_value(
                mcp.call("invite_accounts", json!({"input": payload}))
                    .await?,
            )?;
            if result.request_id != payload.request_id
                || result.url.origin() != origin.origin()
                || !result.url.username().is_empty()
                || result.url.password().is_some()
            {
                return Err("invalid invitation receipt".into());
            }
            Ok(Screen::new(
                t(
                    "Invitation created. Open the private, one-time message on the website. If delivery failed, check accounts before retrying.",
                    "تم إنشاء الدعوة. افتح الرسالة الخاصة لمرة واحدة على الموقع. إذا فشل التسليم فتحقق من الحسابات قبل إعادة المحاولة.",
                ),
                vec![
                    vec![Button::Link(
                        t("Open private invitation", "فتح الدعوة الخاصة").into(),
                        result.url,
                    )],
                    vec![back(lang, list), home(lang)],
                ],
            ))
        }
        _ => Ok(unavailable(lang)),
    }
}

pub(super) async fn add_student(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: &Notebook,
) -> Result<Screen> {
    let course = action.value("courseId");
    let (tool, destination) = if course.is_empty() {
        ("save_student", Action::new("nb.students"))
    } else if notebook.courses.iter().any(|item| item.id == course) {
        (
            "create_student_in_course",
            Action::new("co.members").arg("id", course),
        )
    } else {
        return Ok(unavailable(lang));
    };
    if !tools.contains(tool) {
        return Ok(unavailable(lang));
    }
    let rows = || vec![vec![back(lang, destination.clone()), home(lang)]];
    if action.route == "nb.add" || input.is_none() {
        let next = Action::new("nb.add.save")
            .arg("courseId", course)
            .arg("studentId", uuid()?)
            .arg("enrollmentId", uuid()?);
        return Ok(Screen::new(
            lang.text("Send the student name.", "أرسل اسم الطالب."),
            rows(),
        )
        .prompt(next));
    }
    let raw = input.unwrap_or_default();
    let name = if action.route == "nb.add.save" {
        raw
    } else {
        action.value("name")
    };
    if name.is_empty() || name.encode_utf16().count() > 200 {
        let retry = action.clone().arg("name", "");
        return Ok(Screen::new(
            lang.text(
                "Enter a name of 1–200 characters.",
                "أدخل اسماً من ١ إلى ٢٠٠ حرف.",
            ),
            rows(),
        )
        .prompt(retry));
    }
    if action.route == "nb.add.save" {
        let mut next = action.clone().arg("name", name);
        next.route = "nb.add.level".into();
        return Ok(Screen::new(
            lang.text(
                "Send the initial student level, or - for blank.",
                "أرسل مستوى الطالب الأولي، أو - لتركه فارغاً.",
            ),
            rows(),
        )
        .prompt(next));
    }
    let level = if raw == "-" { "" } else { raw };
    if level.encode_utf16().count() > 100 {
        return Ok(Screen::new(
            lang.text(
                "Level must be at most 100 characters.",
                "المستوى حتى ١٠٠ حرف.",
            ),
            rows(),
        )
        .prompt(action.clone()));
    }
    if !id_shape(action.value("studentId")) {
        return Ok(unavailable(lang));
    }
    let student = json!({"studentId": action.value("studentId"), "name": name, "teacherId": null, "group": level, "active": true});
    let input = if course.is_empty() {
        student
    } else if id_shape(action.value("enrollmentId")) {
        json!({"student":student, "enrollment": {"id":action.value("enrollmentId"),"courseId":course}})
    } else {
        return Ok(unavailable(lang));
    };
    mcp.call(tool, json!({"input":input})).await?;
    Ok(Screen::new(
        lang.text("Student saved.", "تم حفظ الطالب."),
        rows(),
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StudentVersion {
    first_name: String,
    last_name: String,
    group: String,
    whatsapp: Option<String>,
    location: Option<String>,
    birth_year: Option<i32>,
}

pub(super) async fn edit_student(
    mcp: &Mcp<'_>,
    tools: &HashSet<String>,
    lang: Language,
    action: &Action,
    input: Option<&str>,
    notebook: &Notebook,
) -> Result<Screen> {
    if !tools.contains("update_student") {
        return Ok(unavailable(lang));
    }
    let Some(student) = notebook
        .students
        .iter()
        .find(|student| student.student_id == action.value("studentId") && student.active)
    else {
        return Ok(unavailable(lang));
    };
    let target = |route| Action::new(route).arg("studentId", &student.student_id);
    let back_rows = || vec![vec![back(lang, target("nb.student")), home(lang)]];
    let fields = [
        ("firstName", lang.text("First name", "الاسم الأول")),
        ("lastName", lang.text("Last name", "اسم العائلة")),
        ("whatsapp", lang.text("Phone", "الهاتف")),
        ("location", lang.text("Location", "الموقع")),
        ("birthYear", lang.text("Birth year", "سنة الميلاد")),
    ];
    if action.route == "nb.edit" {
        let mut rows: Vec<_> = fields
            .iter()
            .map(|(id, label)| vec![choice(label, target("nb.phone").arg("field", *id))])
            .collect();
        rows.extend(back_rows());
        return Ok(Screen::new(
            lang.text("Choose a field to edit.", "اختر حقلاً للتعديل."),
            rows,
        ));
    }
    let field = action
        .args
        .get("field")
        .map(String::as_str)
        .unwrap_or("whatsapp");
    if !fields.iter().any(|(key, _)| *key == field) {
        return Ok(unavailable(lang));
    }
    if action.route == "nb.phone" || input.is_none() {
        let current = StudentVersion {
            first_name: student.first_name.as_ref().unwrap_or(&student.name).clone(),
            last_name: student.last_name.clone().unwrap_or_default(),
            group: student.group.clone(),
            whatsapp: student.whatsapp.clone(),
            location: student.location.clone(),
            birth_year: student.birth_year,
        };
        let next = target("nb.phone.save")
            .arg("field", field)
            .arg("expected", serde_json::to_string(&current)?);
        return Ok(Screen::new(
            lang.text(
                "Send the new value, or - to clear an optional field.",
                "أرسل القيمة الجديدة، أو - لمسح حقل اختياري.",
            ),
            back_rows(),
        )
        .prompt(next));
    }
    let previous: StudentVersion = serde_json::from_str(action.value("expected"))?;
    let raw = input.unwrap_or_default();
    let next = if raw == "-" {
        match field {
            "firstName" => None,
            "lastName" => Some(json!("")),
            _ => Some(Value::Null),
        }
    } else {
        match field {
            "birthYear" => raw
                .parse::<i32>()
                .ok()
                .filter(|year| (1900..=Utc::now().year()).contains(year))
                .map(|year| json!(year)),
            "whatsapp" if raw.encode_utf16().count() <= 40 => Some(json!(raw)),
            "firstName" | "lastName" | "location"
                if !raw.is_empty() && raw.encode_utf16().count() <= 200 =>
            {
                Some(json!(raw))
            }
            _ => None,
        }
    };
    let Some(next) = next else {
        return Ok(Screen::new(
            lang.text("Invalid value; try again.", "القيمة غير صالحة؛ حاول مجدداً."),
            back_rows(),
        )
        .prompt(action.clone()));
    };
    let mut payload = serde_json::to_value(&previous)?;
    payload[field] = next;
    payload["studentId"] = json!(student.student_id);
    payload["expected"] = serde_json::to_value(previous)?;
    mcp.call("update_student", payload).await?;
    Ok(Screen::new(
        lang.text("Student updated.", "تم تحديث الطالب."),
        back_rows(),
    ))
}
