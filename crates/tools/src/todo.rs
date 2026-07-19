use crate::{Tool, ToolError};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus { Pending, InProgress, Done, Failed }

#[derive(Debug, Clone)]
pub struct TodoItem { pub text: String, pub status: TodoStatus }

/// Parse a `{ "items": [ { "text", "status" } ] }` payload into plan items.
/// Unknown/missing status -> Pending; non-object/absent items -> empty.
pub fn parse_todos(args: &Value) -> Vec<TodoItem> {
    let Some(items) = args.get("items").and_then(|v| v.as_array()) else { return Vec::new() };
    items.iter().filter_map(|it| {
        let text = it.get("text")?.as_str()?.to_string();
        let status = match it.get("status").and_then(|s| s.as_str()).unwrap_or("pending") {
            "in_progress" => TodoStatus::InProgress,
            "done" => TodoStatus::Done,
            "failed" => TodoStatus::Failed,
            _ => TodoStatus::Pending,
        };
        Some(TodoItem { text, status })
    }).collect()
}

/// The `todo` tool: the model publishes/updates its plan. The TUI reads the same
/// payload off `ToolStarted` to render the plan pane; this just validates + acks.
pub struct TodoTool;

#[async_trait::async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str { "todo" }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type":"function",
            "function":{
                "name":"todo",
                "description":"Publish or update your task plan. Call it with the FULL list each time; set each item's status to pending/in_progress/done/failed as you work.",
                "parameters":{"type":"object","properties":{"items":{"type":"array","items":{
                    "type":"object",
                    "properties":{"text":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","done","failed"]}},
                    "required":["text","status"]}}},"required":["items"]}
            }
        })
    }
    async fn call(&self, args: Value) -> Result<String, ToolError> {
        Ok(format!("plan: {} item(s)", parse_todos(&args).len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_items_and_statuses() {
        let v = serde_json::json!({"items":[
            {"text":"read","status":"done"},
            {"text":"map","status":"in_progress"},
            {"text":"add","status":"pending"},
            {"text":"weird","status":"???"}
        ]});
        let items = parse_todos(&v);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].status, TodoStatus::Done);
        assert_eq!(items[1].status, TodoStatus::InProgress);
        assert_eq!(items[3].status, TodoStatus::Pending); // unknown -> pending
        assert_eq!(items[0].text, "read");
    }
    #[test]
    fn parse_bad_json_is_empty() {
        assert!(parse_todos(&serde_json::json!({"nope":1})).is_empty());
    }
    #[tokio::test]
    async fn todo_tool_confirms_count() {
        let out = TodoTool.call(serde_json::json!({"items":[{"text":"a","status":"pending"}]})).await.unwrap();
        assert!(out.contains('1'));
    }
}
