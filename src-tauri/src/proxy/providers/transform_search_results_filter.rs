//! 代理响应侧的空搜索结果过滤器（Claude 透传分支专用）。
//!
//! 背景：部分中转渠道不支持 Claude Code 的 WebSearch 工具，会把联网搜索
//! "假支持"成一个注入 assistant 内容的 text block，内容形如
//! `"Search results for query: <可能为空>"`。冒号后没有实质内容的空搜索头
//! 对模型是纯噪声（还会在历史里诱导复读），需要在代理响应侧整块丢弃。
//!
//! 判定规则与请求侧清理共用 `claude_web_search`：
//! - trim 后以 `Search results for query:` 开头且冒号后纯空白 → 整块丢弃；
//! - 冒号后有内容 → 原样保留（不改写）；
//! - 不匹配的 text block 一律透传。
//!
//! 已接受的代价：流式过滤需要把整个 text block 缓冲到 `content_block_stop`
//! 才能判定去留，因此被缓冲 block 的打字机效果会延迟到该 block 结束后
//! 一次性发出（start + 合并 delta + stop）。非 text block（thinking /
//! tool_use 等）与 message 级事件不受影响，仍然即时转发。

use crate::proxy::sse::{append_utf8_safe, strip_sse_field, take_sse_block};
use bytes::Bytes;use futures::stream::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;

/// 从非流式 Anthropic Messages 响应 JSON 的 `content[]` 中剔除空搜索结果头
/// text block；content 被清空时补占位 block 保证非空。body 不是 Messages
/// 响应形状（无 content 数组）时不动。返回是否有改动。
pub(crate) fn filter_empty_search_result_blocks_in_response(response: &mut Value) -> bool {
    let Some(content) = response.get_mut("content").and_then(Value::as_array_mut) else {
        return false;
    };
    crate::claude_web_search::strip_empty_search_result_text_blocks(content)
}

/// 被缓冲中的 text block：start 事件已吞下，文本随 delta 累积，
/// 到 `content_block_stop` 时判定整块去留。
struct PendingTextBlock {
    /// 上游原始 index（丢弃/保留判定与 index 重编号要用）
    original_index: u64,
    /// 完整的 content_block_start 事件 JSON（保留 content_block 里的其它字段）
    start_event: Value,
    /// start 事件的 SSE `event:` 行内容（原样保留，通常为 content_block_start）
    start_event_name: Option<String>,
    /// 已累积的 text_delta 文本
    text: String,
}

/// SSE 过滤状态机。逐 SSE block 处理，输出零个或多个待转发 block。
///
/// index 重编号：转发的 content_block_* 事件携带 `index` 字段，被丢弃的
/// block 会造成空洞。维护 original index → 新 index 映射，保证转发出去的
/// 序列从 0 连续编号；index 未变的事件原样透传（字节级一致），只在确实
/// 发生重编号时才重写 `index` 并重序列化。
#[derive(Default)]
struct SearchResultsSseFilter {
    pending: Option<PendingTextBlock>,
    index_map: HashMap<u64, u64>,
    next_index: u64,
}

impl SearchResultsSseFilter {
    /// 处理一个完整 SSE block（不含结尾空行），返回要转发的字节块序列。
    fn process_block(&mut self, block: &str) -> Vec<Bytes> {
        if block.trim().is_empty() {
            return vec![];
        }

        let mut event_name: Option<&str> = None;
        let mut data_parts: Vec<&str> = Vec::new();
        for line in block.lines() {
            if let Some(name) = strip_sse_field(line, "event") {
                event_name = Some(name.trim());
            }
            if let Some(data) = strip_sse_field(line, "data") {
                data_parts.push(data);
            }
        }

        // 无 data / [DONE] / 非 JSON data：原样透传
        if data_parts.is_empty() {
            return vec![verbatim(block)];
        }
        let data = data_parts.join("\n");
        if data.trim() == "[DONE]" {
            return vec![verbatim(block)];
        }
        let event: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => return vec![verbatim(block)],
        };

        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "content_block_start" => self.handle_start(block, event_name, event),
            "content_block_delta" => self.handle_delta(block, event_name, event),
            "content_block_stop" => self.handle_stop(block, event_name, event),
            // message_start 标志新 message：index 序列重新从 0 开始
            // （单条响应正常只有一个 message，这里是防御性重置）
            "message_start" => {
                let mut out = self.finalize_pending();
                self.index_map.clear();
                self.next_index = 0;
                out.push(verbatim(block));
                out
            }
            // message_delta / message_stop / ping / error 等：即时原样转发
            _ => vec![verbatim(block)],
        }
    }

    /// content_block_start：text block 开始缓冲（吞掉 start）；其它类型
    /// 即时转发并登记 index 映射。
    fn handle_start(&mut self, block: &str, event_name: Option<&str>, mut event: Value) -> Vec<Bytes> {
        // 防御：上一个 text block 未收到 stop 又来了新 block，先按"保留"收尾
        let mut out = self.finalize_pending();

        let original_index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        let block_type = event
            .pointer("/content_block/type")
            .and_then(Value::as_str)
            .unwrap_or("");

        if block_type == "text" {
            self.pending = Some(PendingTextBlock {
                original_index,
                start_event: event,
                start_event_name: event_name.map(str::to_string),
                text: String::new(),
            });
        } else {
            let new_index = self.assign_index(original_index);
            out.push(rewrite_or_verbatim(
                block,
                event_name,
                &mut event,
                original_index,
                new_index,
            ));
        }
        out
    }

    /// content_block_delta：属于被缓冲 block 的 text_delta 累积进内存不转发；
    /// 其它 delta 过 index 重编号后转发。
    fn handle_delta(&mut self, block: &str, event_name: Option<&str>, mut event: Value) -> Vec<Bytes> {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);

        if let Some(pending) = self.pending.as_mut() {
            if pending.original_index == index {
                if event.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") {
                    if let Some(text) = event.pointer("/delta/text").and_then(Value::as_str) {
                        pending.text.push_str(text);
                    }
                }
                return vec![];
            }
            // 防御：delta 不属于被缓冲 block，先收尾再转发
            let mut out = self.finalize_pending();
            out.push(self.rewrite_indexed_event(block, event_name, &mut event, index));
            return out;
        }

        vec![self.rewrite_indexed_event(block, event_name, &mut event, index)]
    }

    /// content_block_stop：被缓冲 block 在此判定去留；其它 stop 过 index
    /// 重编号后转发。
    fn handle_stop(&mut self, block: &str, event_name: Option<&str>, mut event: Value) -> Vec<Bytes> {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);

        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.original_index == index)
        {
            let pending = self.pending.take().expect("checked above");
            if crate::claude_web_search::is_empty_search_results_header(&pending.text) {
                // 空搜索头：整块丢弃（start/deltas/stop 都不发），不消耗新 index
                return vec![];
            }
            // 有实质内容：start + 一个合并的 delta + stop 一次性发出
            return self.emit_buffered_block(pending);
        }

        if self.pending.is_some() {
            // 防御：stop 不属于被缓冲 block，先收尾再转发
            let mut out = self.finalize_pending();
            out.push(self.rewrite_indexed_event(block, event_name, &mut event, index));
            return out;
        }

        vec![self.rewrite_indexed_event(block, event_name, &mut event, index)]
    }

    /// 把缓冲的 text block 以连续新 index 发出：start（保留原事件的其它
    /// 字段）+ 合并为单个 text_delta + stop。
    fn emit_buffered_block(&mut self, pending: PendingTextBlock) -> Vec<Bytes> {
        let new_index = self.assign_index(pending.original_index);

        let mut start = pending.start_event;
        start["index"] = json!(new_index);
        let delta = json!({
            "type": "content_block_delta",
            "index": new_index,
            "delta": { "type": "text_delta", "text": pending.text }
        });
        let stop = json!({ "type": "content_block_stop", "index": new_index });

        vec![
            serialize_event(pending.start_event_name.as_deref(), &start),
            serialize_event(Some("content_block_delta"), &delta),
            serialize_event(Some("content_block_stop"), &stop),
        ]
    }

    /// 防御性收尾：流中断/乱序导致被缓冲 block 等不到 stop 时按"保留"发出。
    /// 此时无法排除后续文本会让搜索头"有内容"，宁可透传也不丢内容。
    fn finalize_pending(&mut self) -> Vec<Bytes> {
        let Some(pending) = self.pending.take() else {
            return vec![];
        };
        self.emit_buffered_block(pending)
    }

    /// 登记 original index → 新 index 映射，新 index 从 0 连续分配
    /// （被丢弃的 block 不消耗序号）。
    fn assign_index(&mut self, original_index: u64) -> u64 {
        let new_index = self.next_index;
        self.next_index += 1;
        self.index_map.insert(original_index, new_index);
        new_index
    }

    /// 非缓冲 content_block 事件的 index 重编号：映射存在且发生变化时重写
    /// `index` 重序列化，否则原样透传。
    fn rewrite_indexed_event(
        &self,
        block: &str,
        event_name: Option<&str>,
        event: &mut Value,
        original_index: u64,
    ) -> Bytes {
        match self.index_map.get(&original_index) {
            Some(&new_index) => rewrite_or_verbatim(block, event_name, event, original_index, new_index),
            // 映射缺失（异常流）：原样透传，不重编号
            None => verbatim(block),
        }
    }
}

/// index 未变时原样透传（字节级一致），变了才重写 `index` 重序列化。
fn rewrite_or_verbatim(
    block: &str,
    event_name: Option<&str>,
    event: &mut Value,
    original_index: u64,
    new_index: u64,
) -> Bytes {
    if new_index == original_index {
        return verbatim(block);
    }
    event["index"] = json!(new_index);
    serialize_event(event_name, event)
}

/// 原样转发：保留原始 block 文本，只补回 `\n\n` 分隔符。
fn verbatim(block: &str) -> Bytes {
    Bytes::from(format!("{block}\n\n"))
}

/// 重序列化一个改写过的 SSE 事件（保留 `event:` 行，data 为单行 JSON）。
fn serialize_event(event_name: Option<&str>, event: &Value) -> Bytes {
    let mut out = String::new();
    if let Some(name) = event_name {
        out.push_str("event: ");
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("data: ");
    out.push_str(&serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()));
    out.push_str("\n\n");
    Bytes::from(out)
}

/// 包装上游 Anthropic Messages SSE 字节流，过滤空搜索结果头 text block。
/// 骨架与 `transform_codex_responses_namespace::create_namespace_restore_sse_stream`
/// 一致：逐 chunk 做 UTF-8 边界安全拼接，切出完整 SSE block 后交给状态机。
pub(crate) fn create_search_results_filter_sse_stream<E>(
    stream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + 'static,
{
    async_stream::stream! {
        let mut buffer = String::new();
        let mut utf8_remainder: Vec<u8> = Vec::new();
        let mut filter = SearchResultsSseFilter::default();

        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);
                    while let Some(block) = take_sse_block(&mut buffer) {
                        for out in filter.process_block(&block) {
                            yield Ok(out);
                        }
                    }
                }
                Err(e) => {
                    yield Err(std::io::Error::other(e.to_string()));
                    return;
                }
            }
        }

        // Flush any trailing partial block (streams normally end on a delimiter,
        // but be defensive so no bytes are dropped).
        if !utf8_remainder.is_empty() {
            buffer.push_str(&String::from_utf8_lossy(&utf8_remainder));
        }
        let tail = std::mem::take(&mut buffer);
        if !tail.trim().is_empty() {
            for out in filter.process_block(&tail) {
                yield Ok(out);
            }
        }
        // 防御：流结束时仍有未收到 stop 的缓冲 block，按"保留"收尾
        for out in filter.finalize_pending() {
            yield Ok(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    // ==================== 非流式过滤 ====================

    #[test]
    fn non_streaming_drops_empty_header_keeps_content() {
        let mut response = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "text", "text": "Search results for query:  " },
                { "type": "text", "text": "Search results for query: three hits" },
                { "type": "text", "text": "The answer." }
            ],
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });

        assert!(filter_empty_search_result_blocks_in_response(&mut response));
        let content = response["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["text"], "Search results for query: three hits");
        assert_eq!(content[1]["text"], "The answer.");
        // 其它字段不动
        assert_eq!(response["usage"]["output_tokens"], json!(5));
    }

    #[test]
    fn non_streaming_inserts_placeholder_when_content_emptied() {
        let mut response = json!({
            "content": [{ "type": "text", "text": "Search results for query:" }]
        });

        assert!(filter_empty_search_result_blocks_in_response(&mut response));
        let content = response["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains("omitted"));
    }

    #[test]
    fn non_streaming_noop_without_match_or_content_array() {
        let mut response = json!({
            "content": [{ "type": "text", "text": "Search results for query: hits" }]
        });
        assert!(!filter_empty_search_result_blocks_in_response(&mut response));

        let mut not_a_message = json!({ "error": { "message": "boom" } });
        assert!(!filter_empty_search_result_blocks_in_response(
            &mut not_a_message
        ));
    }

    // ==================== 流式过滤 ====================

    fn sse_event(event_name: &str, data: &str) -> String {
        format!("event: {event_name}\ndata: {data}\n\n")
    }

    /// 构造一条混合 SSE 流：thinking block、空搜索头、有内容搜索头、普通
    /// text block（含中文）各一，原始 index 0-3。
    fn mixed_sse_stream() -> String {
        let mut s = String::new();
        s.push_str(&sse_event(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}"#,
        ));
        // index 0: thinking block（非 text，即时转发）
        s.push_str(&sse_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ));
        // index 1: 空搜索头（应整块丢弃）
        s.push_str(&sse_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Search results for query: "}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":1}"#,
        ));
        // index 2: 有内容搜索头（保留，合并 delta）
        s.push_str(&sse_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Search results for query: "}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"结果一"}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":2}"#,
        ));
        // index 3: 普通 text block（保留）
        s.push_str(&sse_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"你好"}}"#,
        ));
        s.push_str(&sse_event(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":3}"#,
        ));
        s.push_str(&sse_event(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#,
        ));
        s.push_str(&sse_event("message_stop", r#"{"type":"message_stop"}"#));
        s
    }

    async fn run_filter_stream(chunks: Vec<Vec<u8>>) -> String {
        let input = stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, std::io::Error>(Bytes::from(c))),
        );
        let output: Vec<_> = create_search_results_filter_sse_stream(input)
            .collect()
            .await;
        let mut text = String::new();
        for item in output {
            text.push_str(std::str::from_utf8(&item.expect("stream error")).unwrap());
        }
        text
    }

    /// 把输出按 SSE block 切开，提取每个 block 的 (event 行, data JSON)。
    fn parse_sse_blocks(output: &str) -> Vec<(Option<String>, Value)> {
        output
            .split("\n\n")
            .filter(|b| !b.trim().is_empty())
            .map(|block| {
                let mut name = None;
                let mut data = String::new();
                for line in block.lines() {
                    if let Some(n) = strip_sse_field(line, "event") {
                        name = Some(n.trim().to_string());
                    }
                    if let Some(d) = strip_sse_field(line, "data") {
                        data = d.to_string();
                    }
                }
                (name, serde_json::from_str(&data).unwrap())
            })
            .collect()
    }

    #[tokio::test]
    async fn sse_filter_drops_empty_header_and_renumbers_indices() {
        let output = run_filter_stream(vec![mixed_sse_stream().into_bytes()]).await;
        let blocks = parse_sse_blocks(&output);

        let types: Vec<&str> = blocks
            .iter()
            .filter_map(|(_, v)| v.get("type").and_then(Value::as_str))
            .collect();

        // 空搜索头的 start/delta/stop 全部消失（content_block 事件只剩 3 组，
        // index 连续性在下方单独断言）
        assert!(!output.contains(r#""index":3,"#), "got: {output}");
        assert_eq!(
            types,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                // （index 1 空搜索头已消失）
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );

        // content_block_* 事件的 index 连续：0（thinking）、1（有内容搜索头）、2（普通 text）
        let indices: Vec<u64> = blocks
            .iter()
            .filter(|(_, v)| {
                v.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.starts_with("content_block"))
            })
            .map(|(_, v)| v["index"].as_u64().unwrap())
            .collect();
        assert_eq!(indices, vec![0, 0, 0, 1, 1, 1, 2, 2, 2]);

        // 有内容搜索头的文本被合并为单个 delta，内容不变
        let kept_delta = &blocks[5].1;
        assert_eq!(
            kept_delta["delta"]["text"],
            json!("Search results for query: 结果一")
        );
        // 普通 text block 文本不变
        assert_eq!(blocks[8].1["delta"]["text"], json!("你好"));
        // usage / stop_reason 不受影响
        assert_eq!(blocks[10].1["usage"]["output_tokens"], json!(12));
    }

    #[tokio::test]
    async fn sse_filter_preserves_untouched_events_byte_for_byte() {
        let output = run_filter_stream(vec![mixed_sse_stream().into_bytes()]).await;

        // index 未变的事件原样透传：message_start、thinking 三件套、message_delta/stop
        for expected in [
            sse_event(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}"#,
            ),
            sse_event(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            sse_event(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
            ),
            sse_event(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            sse_event(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#,
            ),
            sse_event("message_stop", r#"{"type":"message_stop"}"#),
        ] {
            assert!(output.contains(&expected), "missing {expected:?} in {output}");
        }
    }

    #[tokio::test]
    async fn sse_filter_handles_utf8_split_across_chunks() {
        let full = mixed_sse_stream().into_bytes();
        // 在 "结"（E7 BB 93）内部切断 chunk 边界
        let jie_start = full
            .windows(3)
            .position(|w| w == "结".as_bytes())
            .unwrap();
        let split = jie_start + 1;
        let chunks = vec![full[..split].to_vec(), full[split..].to_vec()];

        let output = run_filter_stream(chunks).await;
        assert!(output.contains("Search results for query: 结果一"));
        // 空搜索头被丢弃后 index 重编号：原始 index 3 不再出现
        assert!(!output.contains(r#""index":3,"#));
    }

    #[tokio::test]
    async fn sse_filter_forwards_error_events_verbatim() {
        let mut input = String::new();
        input.push_str(&sse_event(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ));
        let output = run_filter_stream(vec![input.clone().into_bytes()]).await;
        assert_eq!(output, input);
    }

    #[tokio::test]
    async fn sse_filter_keeps_buffered_block_when_stream_ends_early() {
        // 异常兜底：text block 未收到 stop 流就断了，按"保留"收尾，不丢内容
        let mut input = String::new();
        input.push_str(&sse_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ));
        input.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial answer"}}"#,
        ));
        let output = run_filter_stream(vec![input.into_bytes()]).await;
        assert!(output.contains(r#""type":"content_block_start""#));
        assert!(output.contains("partial answer"));
        assert!(output.contains(r#""type":"content_block_stop""#));
    }

    #[tokio::test]
    async fn sse_filter_empty_stream_with_only_dropped_block() {
        // 罕见兜底：整个 message 只有空搜索头，message 以空内容结束
        // （message_start 之后直接 message_delta/message_stop）
        let mut input = String::new();
        input.push_str(&sse_event(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}"#,
        ));
        input.push_str(&sse_event(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ));
        input.push_str(&sse_event(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Search results for query:"}}"#,
        ));
        input.push_str(&sse_event(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ));
        input.push_str(&sse_event(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
        ));
        input.push_str(&sse_event("message_stop", r#"{"type":"message_stop"}"#));

        let output = run_filter_stream(vec![input.into_bytes()]).await;
        let blocks = parse_sse_blocks(&output);
        let types: Vec<&str> = blocks
            .iter()
            .filter_map(|(_, v)| v.get("type").and_then(Value::as_str))
            .collect();
        assert_eq!(types, vec!["message_start", "message_delta", "message_stop"]);
    }

    #[test]
    fn filter_keeps_text_block_with_empty_text() {
        // 空 text block（不匹配搜索头前缀）按"保留"处理
        let mut filter = SearchResultsSseFilter::default();
        let start = r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let stop = r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#;

        assert!(filter.process_block(start).is_empty());
        let out = filter.process_block(stop);
        assert_eq!(out.len(), 3, "start + merged delta + stop");
    }
}
