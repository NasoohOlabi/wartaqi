use serde_json::{Value, json};

use crate::ledger::{js_trim, valid_uuid};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Student,
    Teacher,
    Administrator,
}

pub(crate) struct Access {
    pub role: Role,
    pub read_scope: bool,
    pub write_scope: bool,
    pub can_read: bool,
    pub can_write: bool,
    pub ready: bool,
    pub user_id: String,
    pub name: String,
}

pub(crate) struct Operation {
    pub path: &'static str,
    pub method: &'static str,
    pub body: Option<Value>,
    pub response: ResponseMapping,
}

#[derive(PartialEq, Eq)]
pub(crate) enum ResponseMapping {
    Direct,
    Courses,
    History(Value),
    SavedCount(usize),
}

// This inventory contains only tools whose handlers are implemented below.
// Account handoffs, reports, workflows and the remaining writes still need ports.
fn tools(access: &Access) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut result = Vec::new();
    if access.read_scope {
        result.extend([
            (
                "get_account",
                "Get the current application account and access permissions.",
                "/api/me",
            ),
            (
                "get_profile",
                "Get your profile name and image.",
                "/api/mcp/profile",
            ),
        ]);
    }
    if !access.ready {
        return result;
    }
    if access.role == Role::Student {
        if access.read_scope {
            result.push((
                "get_my_attendance",
                "Get your own projected attendance, including course names.",
                "/api/student/attendance",
            ));
        }
        return result;
    }
    if access.read_scope && access.can_read {
        result.push((
            "list_courses",
            "List accessible courses and whether they are open.",
            "/api/notebook",
        ));
        result.push(("get_notebook", "Get your authorized roster, courses, enrollments, event history and notes. Attendance events use enrollment IDs to identify courses.", "/api/notebook"));
    }
    if access.write_scope && access.can_write {
        result.push((
            "update_student",
            "Update an active assigned student’s name and contact details.",
            "/api/student/update",
        ));
        result.push(("append_events", "Append attendance, tagged notes or corrections. Supply stable IDs and client times for safe retries. New attendance must reference an active enrollment in an open course.", "/api/events"));
        if access.read_scope && access.can_read {
            result.push(("sync_attendance", "Compare and commit attendance records. Supply the expected event ID (or null) and stable event ID/time; returns per-record conflicts.", "/api/attendance/sync"));
        }
    }
    if access.role == Role::Administrator
        && access.can_read
        && access.can_write
        && access.read_scope
    {
        result.push(("get_notebook_history", "Read administrator notebook history in server order, newest first. Reversals retain their target and undone records remain visible.", "/api/notebook"));
        result.push((
            "get_admin_board",
            "Get accounts, teachers and full student register for administration.",
            "/api/admin",
        ));
    }
    result
}

fn student_schema(year: i32) -> Value {
    let uuid_pattern = "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$";
    let expected = json!({
        "type": "object",
        "properties": {
            "firstName": {"type": "string"}, "lastName": {"type": "string"},
            "group": {"type": "string"}, "whatsapp": {"type": ["string", "null"]},
            "location": {"type": ["string", "null"]},
            "birthYear": {"anyOf": [{"type": "integer", "minimum": -9007199254740991i64, "maximum": 9007199254740991i64}, {"type": "null"}]}
        },
        "required": ["firstName", "lastName", "group", "whatsapp", "location", "birthYear"],
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "properties": {
            "firstName": {"type": "string", "minLength": 1, "maxLength": 200},
            "lastName": {"type": "string", "maxLength": 200},
            "group": {"type": "string", "maxLength": 100},
            "whatsapp": {"anyOf": [{"type": "string", "maxLength": 40}, {"type": "null"}]},
            "location": {"anyOf": [{"type": "string", "maxLength": 200}, {"type": "null"}]},
            "birthYear": {"anyOf": [{"type": "integer", "minimum": 1900, "maximum": year}, {"type": "null"}]},
            "studentId": {"type": "string", "format": "uuid", "pattern": uuid_pattern},
            "expected": expected
        },
        "required": ["firstName", "lastName", "group", "whatsapp", "location", "birthYear", "studentId", "expected"],
        "$schema": "http://json-schema.org/draft-07/schema#",
        "additionalProperties": false
    })
}

fn write_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "append_events" => {
            r#"
{
  "type": "object",
  "properties": {
    "events": {
      "minItems": 1,
      "maxItems": 500,
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id": {
            "type": "string",
            "format": "uuid",
            "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
          },
          "clientTime": {
            "type": "string",
            "format": "date-time",
            "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))T(?:(?:[01]\\d|2[0-3]):[0-5]\\d:[0-5]\\d(?:\\.\\d+)?(?:Z))$"
          },
          "payload": {
            "oneOf": [
              {
                "type": "object",
                "properties": {
                  "kind": {
                    "type": "string",
                    "const": "student"
                  },
                  "studentId": {
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                  },
                  "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 200
                  },
                  "group": {
                    "type": "string",
                    "maxLength": 100
                  },
                  "active": {
                    "type": "boolean"
                  }
                },
                "required": [
                  "kind",
                  "studentId",
                  "name",
                  "group",
                  "active"
                ]
              },
              {
                "type": "object",
                "properties": {
                  "kind": {
                    "type": "string",
                    "const": "attendance"
                  },
                  "studentId": {
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                  },
                  "enrollmentId": {
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                  },
                  "date": {
                    "type": "string",
                    "format": "date",
                    "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))$"
                  },
                  "status": {
                    "anyOf": [
                      {
                        "type": "string",
                        "enum": [
                          "present",
                          "absent",
                          "excused",
                          "early",
                          "late"
                        ]
                      },
                      {
                        "type": "null"
                      }
                    ]
                  }
                },
                "required": [
                  "kind",
                  "studentId",
                  "date",
                  "status"
                ]
              },
              {
                "type": "object",
                "properties": {
                  "kind": {
                    "type": "string",
                    "const": "note"
                  },
                  "studentId": {
                    "anyOf": [
                      {
                        "type": "string",
                        "format": "uuid",
                        "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                      },
                      {
                        "type": "string",
                        "const": ""
                      }
                    ]
                  },
                  "date": {
                    "type": "string",
                    "format": "date",
                    "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))$"
                  },
                  "tags": {
                    "minItems": 1,
                    "maxItems": 20,
                    "type": "array",
                    "items": {
                      "type": "string",
                      "minLength": 1,
                      "maxLength": 60
                    }
                  },
                  "text": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 10000
                  }
                },
                "required": [
                  "kind",
                  "studentId",
                  "date",
                  "tags",
                  "text"
                ]
              },
              {
                "type": "object",
                "properties": {
                  "kind": {
                    "type": "string",
                    "const": "reversal"
                  },
                  "targetId": {
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                  },
                  "reason": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 1000
                  }
                },
                "required": [
                  "kind",
                  "targetId",
                  "reason"
                ]
              }
            ]
          }
        },
        "required": [
          "id",
          "clientTime",
          "payload"
        ]
      }
    }
  },
  "required": [
    "events"
  ],
  "$schema": "http://json-schema.org/draft-07/schema#"
}
"#
        }
        "sync_attendance" => {
            r#"
{
  "type": "object",
  "properties": {
    "records": {
      "minItems": 1,
      "maxItems": 500,
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "event": {
            "type": "object",
            "properties": {
              "id": {
                "type": "string",
                "format": "uuid",
                "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
              },
              "clientTime": {
                "type": "string",
                "format": "date-time",
                "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))T(?:(?:[01]\\d|2[0-3]):[0-5]\\d:[0-5]\\d(?:\\.\\d+)?(?:Z))$"
              },
              "payload": {
                "type": "object",
                "properties": {
                  "kind": {
                    "type": "string",
                    "const": "attendance"
                  },
                  "studentId": {
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                  },
                  "enrollmentId": {
                    "type": "string",
                    "format": "uuid",
                    "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
                  },
                  "date": {
                    "type": "string",
                    "format": "date",
                    "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))$"
                  },
                  "status": {
                    "type": "string",
                    "enum": [
                      "present",
                      "absent",
                      "excused"
                    ]
                  }
                },
                "required": [
                  "kind",
                  "studentId",
                  "enrollmentId",
                  "date",
                  "status"
                ]
              }
            },
            "required": [
              "id",
              "clientTime",
              "payload"
            ]
          },
          "expectedEventId": {
            "anyOf": [
              {
                "type": "string",
                "format": "uuid",
                "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
              },
              {
                "type": "null"
              }
            ]
          },
          "expectedRevision": {
            "type": "string",
            "const": "unknown"
          }
        },
        "required": [
          "event",
          "expectedEventId"
        ]
      }
    }
  },
  "required": [
    "records"
  ],
  "$schema": "http://json-schema.org/draft-07/schema#"
}
"#
        }
        "get_notebook_history" => {
            r#"
{
  "type": "object",
  "properties": {
    "courseId": {
      "anyOf": [
        {
          "type": "string",
          "format": "uuid",
          "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
        },
        {
          "type": "string",
          "const": "none"
        }
      ]
    },
    "studentId": {
      "type": "string",
      "format": "uuid",
      "pattern": "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$"
    },
    "kind": {
      "type": "string",
      "enum": [
        "student",
        "attendance",
        "note",
        "reversal"
      ]
    },
    "from": {
      "type": "string",
      "format": "date",
      "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))$"
    },
    "to": {
      "type": "string",
      "format": "date",
      "pattern": "^(?:(?:\\d\\d[2468][048]|\\d\\d[13579][26]|\\d\\d0[48]|[02468][048]00|[13579][26]00)-02-29|\\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\\d|30)|(?:02)-(?:0[1-9]|1\\d|2[0-8])))$"
    },
    "offset": {
      "default": 0,
      "type": "integer",
      "minimum": 0,
      "maximum": 9007199254740991
    },
    "limit": {
      "default": 50,
      "type": "integer",
      "minimum": 1,
      "maximum": 100
    }
  },
  "required": [
    "courseId"
  ],
  "$schema": "http://json-schema.org/draft-07/schema#"
}
"#
        }
        _ => return None,
    };
    serde_json::from_str(schema).ok()
}

pub(crate) fn discovery(access: &Access, year: i32) -> Vec<Value> {
    tools(access).into_iter().map(|(name, description, _)| {
        if name == "update_student" {
            json!({"name": name, "description": description, "inputSchema": student_schema(year), "execution": {"taskSupport": "forbidden"}})
        } else if name == "get_notebook_history" {
            json!({"name": name, "description": description, "inputSchema": write_schema(name), "annotations": {"readOnlyHint": true}, "execution": {"taskSupport": "forbidden"}})
        } else if let Some(schema) = write_schema(name) {
            json!({"name": name, "description": description, "inputSchema": schema, "execution": {"taskSupport": "forbidden"}})
        } else {
            json!({"name": name, "description": description, "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": true}, "execution": {"taskSupport": "forbidden"}})
        }
    }).collect()
}

const FIELDS: [&str; 6] = [
    "firstName",
    "lastName",
    "group",
    "whatsapp",
    "location",
    "birthYear",
];

fn normalize_fields(input: &Value, update: bool, year: i32) -> Result<Value, ()> {
    let fields = input.as_object().ok_or(())?;
    let mut output = serde_json::Map::new();
    for field in FIELDS {
        let value = fields.get(field).ok_or(())?;
        if field == "birthYear" {
            if value.is_null() {
                output.insert(field.into(), Value::Null);
                continue;
            }
            let number = value
                .as_f64()
                .filter(|n| n.fract() == 0.0 && n.abs() <= 9007199254740991.0)
                .ok_or(())?;
            if update && !(1900.0..=f64::from(year)).contains(&number) {
                return Err(());
            }
            output.insert(field.into(), json!(number as i64));
            continue;
        }
        if value.is_null() && matches!(field, "whatsapp" | "location") {
            output.insert(field.into(), Value::Null);
            continue;
        }
        let text = value.as_str().ok_or(())?;
        let text = if field == "location" && !update {
            text
        } else {
            js_trim(text)
        };
        let length = text.encode_utf16().count();
        if update
            && (length
                > match field {
                    "group" => 100,
                    "whatsapp" => 40,
                    _ => 200,
                }
                || (field == "firstName" && length == 0))
        {
            return Err(());
        }
        output.insert(field.into(), json!(text));
    }
    Ok(Value::Object(output))
}

pub(crate) fn operation(
    access: &Access,
    name: &str,
    arguments: &Value,
    year: i32,
) -> Result<Operation, ()> {
    let (_, _, path) = tools(access)
        .into_iter()
        .find(|(tool, _, _)| *tool == name)
        .ok_or(())?;
    if !arguments.is_object() {
        return Err(());
    }
    if name == "get_notebook_history" {
        return Ok(Operation {
            path,
            method: "GET",
            body: None,
            response: ResponseMapping::History(history_filter(arguments)?),
        });
    }
    if matches!(name, "append_events" | "sync_attendance") {
        return attendance_operation(access, name, arguments, path);
    }
    if name != "update_student" {
        return Ok(Operation {
            path,
            method: "GET",
            body: None,
            response: if name == "list_courses" {
                ResponseMapping::Courses
            } else {
                ResponseMapping::Direct
            },
        });
    }
    let input = arguments.as_object().ok_or(())?;
    if input.len() != 8
        || input
            .keys()
            .any(|key| !FIELDS.contains(&key.as_str()) && key != "studentId" && key != "expected")
    {
        return Err(());
    }
    let id = input
        .get("studentId")
        .and_then(Value::as_str)
        .filter(|id| valid_uuid(id))
        .ok_or(())?;
    let expected = input.get("expected").and_then(Value::as_object).ok_or(())?;
    if expected.len() != FIELDS.len() || expected.keys().any(|key| !FIELDS.contains(&key.as_str()))
    {
        return Err(());
    }
    let expected = normalize_fields(&Value::Object(expected.clone()), false, year)?;
    let mut body = normalize_fields(arguments, true, year)?;
    let first = body["firstName"].as_str().ok_or(())?;
    let last = body["lastName"].as_str().ok_or(())?;
    if first.encode_utf16().count()
        + if last.is_empty() {
            0
        } else {
            1 + last.encode_utf16().count()
        }
        > 200
    {
        return Err(());
    }
    body["studentId"] = json!(id);
    body["expected"] = expected;
    Ok(Operation {
        path,
        method: "POST",
        body: Some(body),
        response: ResponseMapping::Direct,
    })
}

fn event(input: &Value, name: &str) -> Result<Value, ()> {
    let source = input.as_object().ok_or(())?;
    let mut event = serde_json::Map::new();
    for key in ["id", "clientTime", "payload"] {
        event.insert(key.into(), source.get(key).ok_or(())?.clone());
    }
    event.insert("actor".into(), json!(name));
    event.insert("receivedAt".into(), json!(""));
    crate::ledger::decode_input(&Value::Object(event)).map_err(|_| ())
}

fn attendance_operation(
    access: &Access,
    name: &str,
    arguments: &Value,
    path: &'static str,
) -> Result<Operation, ()> {
    let key = if name == "append_events" {
        "events"
    } else {
        "records"
    };
    let records = arguments.get(key).and_then(Value::as_array).ok_or(())?;
    if !(1..=500).contains(&records.len()) {
        return Err(());
    }
    let mut parsed = Vec::with_capacity(records.len());
    for record in records {
        if name == "append_events" {
            parsed.push(event(record, &access.name)?);
            continue;
        }
        let input = record.as_object().ok_or(())?;
        let event = event(input.get("event").ok_or(())?, &access.name)?;
        let payload = &event["payload"];
        if payload["kind"] != "attendance"
            || payload.get("enrollmentId").is_none()
            || !matches!(
                payload["status"].as_str(),
                Some("present" | "absent" | "excused")
            )
        {
            return Err(());
        }
        let expected = input.get("expectedEventId").ok_or(())?;
        if !expected.is_null() && !expected.as_str().is_some_and(valid_uuid) {
            return Err(());
        }
        let mut result = json!({"event": event, "expectedEventId": expected});
        if let Some(revision) = input.get("expectedRevision") {
            if revision != "unknown" {
                return Err(());
            }
            result["expectedRevision"] = revision.clone();
        }
        parsed.push(result);
    }
    let body = if name == "append_events" {
        json!({"events": parsed})
    } else {
        json!({"accountId": access.user_id, "records": parsed})
    };
    let response = if name == "append_events" && !access.read_scope {
        ResponseMapping::SavedCount(records.len())
    } else {
        ResponseMapping::Direct
    };
    Ok(Operation {
        path,
        method: "POST",
        body: Some(body),
        response,
    })
}

pub(crate) fn map_success(mapping: &ResponseMapping, value: Value) -> Result<Value, &'static str> {
    match mapping {
        ResponseMapping::Direct => Ok(value),
        ResponseMapping::SavedCount(count) => Ok(json!({"saved": count})),
        ResponseMapping::History(filter) => history(&value, filter),
        ResponseMapping::Courses => {
            let courses = value
                .get("courses")
                .and_then(Value::as_array)
                .ok_or("Invalid response")?;
            let mut result = Vec::with_capacity(courses.len());
            for course in courses {
                let item = course.as_object().ok_or("Invalid response")?;
                result.push(json!({"id": item.get("id").ok_or("Invalid response")?, "name": item.get("name").ok_or("Invalid response")?, "open": item.get("open").ok_or("Invalid response")?}));
            }
            Ok(json!(result))
        }
    }
}

fn history_filter(arguments: &Value) -> Result<Value, ()> {
    let input = arguments.as_object().ok_or(())?;
    let course = input.get("courseId").and_then(Value::as_str).ok_or(())?;
    if course != "none" && !valid_uuid(course) {
        return Err(());
    }
    let mut filter = json!({"courseId":course});
    for key in ["studentId", "kind", "from", "to"] {
        if let Some(value) = input.get(key) {
            let text = value.as_str().ok_or(())?;
            let valid = match key {
                "studentId" => valid_uuid(text),
                "kind" => matches!(text, "student" | "attendance" | "note" | "reversal"),
                _ => crate::ledger::iso_datetime(&format!("{text}T00:00:00Z")),
            };
            if !valid {
                return Err(());
            }
            filter[key] = value.clone();
        }
    }
    for (key, default, max) in [("offset", 0, 9007199254740991u64), ("limit", 50, 100)] {
        let number = if let Some(value) = input.get(key) {
            value
                .as_f64()
                .filter(|v| {
                    v.fract() == 0.0
                        && *v >= if key == "limit" { 1.0 } else { 0.0 }
                        && *v <= max as f64
                })
                .ok_or(())? as u64
        } else {
            default
        };
        filter[key] = json!(number);
    }
    Ok(filter)
}

fn history(snapshot: &Value, filter: &Value) -> Result<Value, &'static str> {
    use std::collections::{HashMap, HashSet};
    let course = filter["courseId"].as_str().ok_or("Invalid response")?;
    let courses = snapshot["courses"].as_array().ok_or("Invalid response")?;
    if course != "none" && !courses.iter().any(|item| item["id"] == course) {
        return Err("Course not found");
    }
    let enrollments = snapshot["enrollments"]
        .as_array()
        .ok_or("Invalid response")?;
    let ids: HashSet<&str> = enrollments
        .iter()
        .filter(|item| item["courseId"] == course)
        .filter_map(|item| item["id"].as_str())
        .collect();
    let events = snapshot["events"].as_array().ok_or("Invalid response")?;
    let mut by_id = HashMap::new();
    let mut unique = Vec::new();
    let mut reversed = HashSet::new();
    for event in events {
        let id = event["id"].as_str().ok_or("Invalid response")?;
        if by_id.contains_key(id) {
            continue;
        }
        if event["payload"]["kind"] == "reversal"
            && let Some(target_id) = event["payload"]["targetId"].as_str()
            && by_id
                .get(target_id)
                .is_some_and(|target: &&Value| target["payload"]["kind"] != "reversal")
        {
            reversed.insert(target_id);
        }
        unique.push(event);
        by_id.insert(id, event);
    }
    let mut rows = Vec::new();
    for event in unique.into_iter().rev() {
        let payload = &event["payload"];
        let target = if payload["kind"] == "reversal" {
            payload["targetId"]
                .as_str()
                .and_then(|id| by_id.get(id).copied())
        } else {
            Some(event)
        };
        let target_payload = target.map(|item| &item["payload"]);
        if let Some(p) = target_payload
            && p["kind"] == "attendance"
        {
            let enrollment = p.get("enrollmentId").and_then(Value::as_str);
            if (course == "none" && enrollment.is_some())
                || (course != "none" && !enrollment.is_some_and(|id| ids.contains(id)))
            {
                continue;
            }
        }
        if let Some(student) = filter.get("studentId")
            && !target_payload
                .is_some_and(|p| p["kind"] != "reversal" && &p["studentId"] == student)
        {
            continue;
        }
        if filter
            .get("kind")
            .is_some_and(|kind| &payload["kind"] != kind)
        {
            continue;
        }
        let date = target_payload
            .and_then(|p| p.get("date"))
            .and_then(Value::as_str);
        if filter
            .get("from")
            .and_then(Value::as_str)
            .is_some_and(|from| date.is_none_or(|day| day < from))
            || filter
                .get("to")
                .and_then(Value::as_str)
                .is_some_and(|to| date.is_none_or(|day| day > to))
        {
            continue;
        }
        rows.push(json!({"event":event,"undone":event["id"].as_str().is_some_and(|id| reversed.contains(id)),"target":if payload["kind"] == "reversal" {target} else {None}}));
    }
    let offset = filter["offset"].as_u64().ok_or("Invalid response")? as usize;
    let limit = filter["limit"].as_u64().ok_or("Invalid response")? as usize;
    Ok(
        json!({"total":rows.len(),"offset":offset,"records":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>()}),
    )
}

pub(crate) fn map_failure(mapping: &ResponseMapping, message: &str) -> String {
    if *mapping == ResponseMapping::Courses {
        "Unable to load courses".to_owned()
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(role: Role, read_scope: bool, write_scope: bool) -> Access {
        Access {
            role,
            read_scope,
            write_scope,
            can_read: true,
            can_write: true,
            ready: true,
            user_id: "user-1".into(),
            name: "Teacher".into(),
        }
    }

    #[test]
    fn discovery_enforces_roles_scopes_and_setup() {
        let student = access(Role::Student, true, true);
        let names: Vec<_> = discovery(&student, 2026)
            .into_iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["get_account", "get_profile", "get_my_attendance"]);
        assert!(operation(&student, "get_notebook", &json!({}), 2026).is_err());
        let mut teacher = access(Role::Teacher, true, false);
        assert!(operation(&teacher, "update_student", &json!({}), 2026).is_err());
        assert_eq!(
            operation(&teacher, "get_notebook", &json!({}), 2026)
                .unwrap()
                .path,
            "/api/notebook"
        );
        teacher.ready = false;
        assert!(operation(&teacher, "get_notebook", &json!({}), 2026).is_err());
        assert_eq!(discovery(&teacher, 2026).len(), 2);
    }

    #[test]
    fn attendance_scopes_and_request_transform() {
        let mut teacher = access(Role::Teacher, true, true);
        let event = json!({"id":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac", "clientTime":"2026-11-27T10:00:00.000Z", "payload":{"kind":"attendance", "studentId":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac", "enrollmentId":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac", "date":"2026-11-27", "status":"present", "ignored":true}, "ignored":true});
        let input = json!({"records":[{"event":event,"expectedEventId":null,"ignored":true}]});
        let request = operation(&teacher, "sync_attendance", &input, 2026).unwrap();
        let body = request.body.unwrap();
        assert_eq!(body["accountId"], "user-1");
        assert_eq!(body["records"][0]["event"]["actor"], "Teacher");
        assert!(body["records"][0]["event"].get("ignored").is_none());
        teacher.read_scope = false;
        assert!(operation(&teacher, "sync_attendance", &input, 2026).is_err());
        let append =
            operation(&teacher, "append_events", &json!({"events":[event]}), 2026).unwrap();
        assert!(append.response == ResponseMapping::SavedCount(1));
        assert_eq!(
            map_success(&append.response, json!({"private":true})).unwrap(),
            json!({"saved":1})
        );
    }

    #[test]
    fn history_resolves_reversal_and_course_scope() {
        let admin = access(Role::Administrator, true, true);
        let course_id = "e4856c4d-a0ad-4f0c-af15-f8f5e24dddac";
        let request = operation(
            &admin,
            "get_notebook_history",
            &json!({"courseId":course_id}),
            2026,
        )
        .unwrap();
        let snapshot = json!({"courses":[{"id":course_id}], "enrollments":[{"id":"entry", "courseId":course_id}], "events":[
            {"id":"event-a","payload":{"kind":"attendance","studentId":"student","enrollmentId":"entry","date":"2026-11-27"}},
            {"id":"event-b","payload":{"kind":"reversal","targetId":"event-a"}}
        ]});
        let result = map_success(&request.response, snapshot).unwrap();
        assert_eq!(result["total"], 2);
        assert_eq!(result["records"][0]["target"]["id"], "event-a");
        assert_eq!(result["records"][1]["undone"], true);
        assert_eq!(
            map_failure(&ResponseMapping::Courses, "ignored"),
            "Unable to load courses"
        );
        assert!(
            operation(
                &access(Role::Teacher, true, true),
                "get_notebook_history",
                &json!({"courseId":course_id}),
                2026
            )
            .is_err()
        );
    }

    #[test]
    fn student_update_validates_and_normalizes() {
        let teacher = access(Role::Teacher, false, true);
        let mut input = json!({"studentId":"e4856c4d-a0ad-4f0c-af15-f8f5e24dddac","firstName":"  Ada  ","lastName":"  Lovelace ","group":" A ","whatsapp": null,"location":" Here ","birthYear":2000,"expected":{"firstName":" Ada ","lastName":"Lovelace","group":"A","whatsapp":null,"location":" Untrimmed ","birthYear":2000}});
        let operation = operation(&teacher, "update_student", &input, 2026).unwrap();
        assert_eq!(operation.method, "POST");
        assert_eq!(operation.body.unwrap()["location"], "Here");
        input["extra"] = json!(true);
        assert!(super::operation(&teacher, "update_student", &input, 2026).is_err());
    }
}
