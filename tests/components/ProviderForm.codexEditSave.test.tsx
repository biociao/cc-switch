import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

const toastMocks = vi.hoisted(() => ({
  error: vi.fn(),
  success: vi.fn(),
  info: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: toastMocks,
}));

import { ProviderForm } from "@/components/providers/forms/ProviderForm";
import { setSettings } from "../msw/state";

// OpenCode Go 供应商的真实 DB 形态（2026-08-14 快照）
const OPENCODE_GO_SETTINGS = {
  auth: {
    OPENAI_API_KEY: "sk-J6xwyMaqPe9NxKkuS7NYGTqPRo8qAeqZ0pkFYodSVmULOCC8xe8ePgAQl7nDXUqn",
  },
  config: `model_provider = "custom"
model = "glm-5.2"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = "opencode_go"
base_url = "https://opencode.ai/zen/go/v1"
wire_api = "responses"
requires_openai_auth = true
`,
  modelCatalog: {
    models: [
      { model: "glm-5.2", displayName: "GLM 5.2", contextWindow: 204800 },
      { model: "glm-5.1", displayName: "GLM 5.1", contextWindow: 204800 },
      { model: "kimi-k2.7-code", displayName: "Kimi K2.7 Code", contextWindow: 262144 },
      { model: "deepseek-v4-pro", displayName: "DeepSeek V4 Pro" },
      { model: "deepseek-v4-flash", displayName: "DeepSeek V4 Flash" },
      { model: "mimo-v2.5-pro", displayName: "MiMo V2.5 Pro", contextWindow: 1048576 },
    ],
  },
};

const renderEditForm = (onSubmit: (values: unknown) => void) => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ProviderForm
        appId="codex"
        providerId="8143d143-91e7-4962-b3d1-e788e3e4b2c1"
        onSubmit={onSubmit}
        onCancel={vi.fn()}
        submitLabel="保存"
        initialData={{
          name: "OpenCode Go",
          websiteUrl: "",
          settingsConfig: OPENCODE_GO_SETTINGS,
          category: "third_party",
          meta: {
            commonConfigEnabled: false,
            endpointAutoSelect: true,
            apiFormat: "openai_responses",
          },
        }}
      />
    </QueryClientProvider>,
  );
};

describe("ProviderForm Codex 编辑模式模型映射保存", () => {
  it("新增一行映射后提交，settingsConfig 应包含全部 7 行", async () => {
    const onSubmit = vi.fn();
    setSettings({ commonConfigConfirmed: true });
    renderEditForm(onSubmit);

    // 等编辑加载完成：6 行映射渲染出来
    await waitFor(() => {
      expect(
        screen.getAllByPlaceholderText("例如: deepseek-v4-flash"),
      ).toHaveLength(6);
    });

    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));

    const modelInputs = await screen.findAllByPlaceholderText(
      "例如: deepseek-v4-flash",
    );
    const displayInputs = screen.getAllByPlaceholderText(
      "例如: DeepSeek V4 Flash",
    );
    expect(modelInputs).toHaveLength(7);

    fireEvent.change(displayInputs[6], { target: { value: "GPT-5.2" } });
    fireEvent.change(modelInputs[6], { target: { value: "qwen3.5-max" } });

    // 提交（保存按钮）
    const buttons = screen.getAllByRole("button").map((b) => b.textContent);
    console.log("BUTTONS:", JSON.stringify(buttons));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const values = onSubmit.mock.calls[0][0] as { settingsConfig: string };
    const saved = JSON.parse(values.settingsConfig) as {
      modelCatalog?: { models: Array<{ model: string; displayName?: string }> };
    };
    expect(saved.modelCatalog?.models.map((m) => m.model)).toEqual([
      "glm-5.2",
      "glm-5.1",
      "kimi-k2.7-code",
      "deepseek-v4-pro",
      "deepseek-v4-flash",
      "mimo-v2.5-pro",
      "qwen3.5-max",
    ]);
    expect(saved.modelCatalog?.models[6].displayName).toBe("GPT-5.2");
  });

  it("新增行只填显示名不填实际请求模型时，拦截保存并报错", async () => {
    const onSubmit = vi.fn();
    setSettings({ commonConfigConfirmed: true });
    renderEditForm(onSubmit);

    await waitFor(() => {
      expect(
        screen.getAllByPlaceholderText("例如: deepseek-v4-flash"),
      ).toHaveLength(6);
    });

    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));
    const displayInputs = await screen.findAllByPlaceholderText(
      "例如: DeepSeek V4 Flash",
    );
    fireEvent.change(displayInputs[6], { target: { value: "GPT-5.2" } });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(toastMocks.error).toHaveBeenCalled());
    expect(toastMocks.error.mock.calls[0][0]).toContain("实际请求模型");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("新增行与已有行的实际请求模型重复时，拦截保存并报错", async () => {
    const onSubmit = vi.fn();
    setSettings({ commonConfigConfirmed: true });
    renderEditForm(onSubmit);

    await waitFor(() => {
      expect(
        screen.getAllByPlaceholderText("例如: deepseek-v4-flash"),
      ).toHaveLength(6);
    });

    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));
    const modelInputs = await screen.findAllByPlaceholderText(
      "例如: deepseek-v4-flash",
    );
    // 与已有行 glm-5.2 重复
    fireEvent.change(modelInputs[6], { target: { value: "glm-5.2" } });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(toastMocks.error).toHaveBeenCalled());
    expect(toastMocks.error.mock.calls[0][0]).toContain("重复");
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
