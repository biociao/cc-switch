## 测试报告：Kimi for Coding 端点兼容性问题（已定位根因并本地验证修复）

### 现象 / Symptom

在本 PR 的开发构建上，使用 **Kimi for Coding**（`https://api.kimi.com/coding`，Anthropic 兼容端点）作为当前 Claude 供应商时，通过工具栏启动器运行 Claude Science，agent 执行即失败：

```
Agent Failed
400 {"error":{"type":"invalid_request_error","message":"tool_call_id  is not found"},"type":"error"}
```

同一会话切换到 GLM（`api.z.ai`）可正常运行，说明是端点对请求形状的严格程度差异，而非本 PR 启动器本身的问题。

### 根因 / Root cause

抓取代理归一化后的实际上行请求体分析发现：Claude Science 的历史消息中携带 **Anthropic 服务端工具块**——assistant 消息内含 `server_tool_use` 和 `web_search_tool_result`（本例中 `content` 为空数组，且 `web_search_tool_result` 没有 `tool_use_id`）。Kimi 网关在将 Anthropic 格式转换为内部 OpenAI 格式时，会把该块映射成找不到对应 `tool_call` 的 tool 消息，于是返回 `tool_call_id  is not found`（报错信息中 id 为空，与此吻合）。

请求中声明的服务端工具定义 `{"type": "web_search_20250305", "name": "web_search"}` 本身被 Kimi 接受，问题只出在历史消息块上。

最小请求形状示例（messages 节选）：

```json
{
  "role": "assistant",
  "content": [
    {"type": "text", "text": "Search results for query: "},
    {"type": "web_search_tool_result", "content": []},
    {"type": "tool_use", "id": "tool_xxx", "name": "list_compute", "input": {}}
  ]
}
```

### 修复方案与验证 / Proposed fix & validation

参考本 PR 中 DeepSeek 专属归一化（`normalize_deepseek_*`）的模式，在 `normalize_anthropic_messages_for_provider` 管道中新增一步：对**非 Anthropic 官方端点**，将历史消息中的 `server_tool_use` 及各 `*_tool_result`（`web_search_tool_result` 等）降级为文本块（内容为空则移除），保留检索结果对模型可见；官方端点原样透传。

本地补丁验证结果：

- 导致失败的原始请求体原样回放 → 修复前 400，修复后 **200**
- Claude Science + Kimi for Coding 实测：agent 任务正常执行 ✅
- GLM 端点回归无影响

### 次要发现 / Secondary finding

排查过程中还发现 Kimi 端点同样严格校验 `tool_use` / `tool_result` 配对：孤儿 `tool_result`（紧邻上一条 assistant 消息中无匹配 `tool_use`，含 Science resume 时在**首条消息**注入 tool_result 的场景）会被同样的 400 拒绝，GLM 则容忍。`copilot_optimizer.rs` 中现成的 `sanitize_orphan_tool_results` 逻辑可推广到 anthropic 透传路径作为防御性清洗（注意其当前实现不覆盖首条消息）。此为独立加固项，与上面的根因修复可分开取舍。

### 环境 / Environment

- cc-switch：本 PR head（`ad1f375`）开发构建，macOS（arm64）
- Claude Science 0.1.18（PR 启动器拉起的隔离 profile）
- 上游端点：`https://api.kimi.com/coding/v1/messages`，模型 `k3`（Kimi for Coding）

---

## Test report: Kimi for Coding endpoint incompatibility (root cause identified, fix verified locally)

### Symptom

On a dev build of this PR, with **Kimi for Coding** (`https://api.kimi.com/coding`, Anthropic-compatible endpoint) as the active Claude provider, launching Claude Science via the new toolbar launcher fails as soon as the agent runs:

```
Agent Failed
400 {"error":{"type":"invalid_request_error","message":"tool_call_id  is not found"},"type":"error"}
```

The same session works with GLM (`api.z.ai`), so this is endpoint strictness, not a launcher bug.

### Root cause

Dumping the normalized upstream request shows Claude Science history carries **Anthropic server-tool blocks**: an assistant message contains `server_tool_use` / `web_search_tool_result` (in this case with empty `content` and no `tool_use_id`). When Kimi's gateway maps Anthropic format to its internal OpenAI format, that block becomes a tool message referencing a non-existent `tool_call`, hence `tool_call_id  is not found` (note the empty id in the message).

The declared server tool `{"type": "web_search_20250305", "name": "web_search"}` is accepted by Kimi; only the history blocks break it.

### Proposed fix & validation

Following the pattern of this PR's DeepSeek-specific normalizers (`normalize_deepseek_*`), add one more step to `normalize_anthropic_messages_for_provider`: for **non-official Anthropic endpoints**, rewrite `server_tool_use` and `*_tool_result` history blocks (`web_search_tool_result`, etc.) into text blocks (dropping empty ones), keeping retrieved content visible to the model. Official `api.anthropic.com` traffic passes through untouched.

Verified locally:

- Replaying the exact captured failing request: 400 before the patch, **200 after**
- Claude Science + Kimi for Coding end-to-end: agent tasks now run ✅
- No regression with GLM

### Secondary finding

Kimi also strictly validates `tool_use` / `tool_result` pairing: orphan `tool_result`s (no matching `tool_use` in the immediately preceding assistant message — including the case where Science injects a tool_result into the **first** message on session resume) are rejected with the same 400, while GLM tolerates them. The existing `sanitize_orphan_tool_results` in `copilot_optimizer.rs` could be generalized to the Anthropic passthrough path as defensive hardening (note it currently doesn't cover the first message). This is an independent, optional hardening item.

### Environment

- cc-switch: dev build at this PR's head (`ad1f375`), macOS arm64
- Claude Science 0.1.18 (isolated profile launched by this PR)
- Upstream: `https://api.kimi.com/coding/v1/messages`, model `k3` (Kimi for Coding)
