import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { Provider } from "@/types";
import { setSettings } from "../msw/state";

// 只 mock 对话框外壳与 live 读取，保留真实 ProviderForm / CodexFormFields 全链路
const apiMocks = vi.hoisted(() => ({
  getCurrent: vi.fn(),
  getLiveProviderSettings: vi.fn(),
}));

vi.mock("@/lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...original,
    providersApi: {
      ...original.providersApi,
      getCurrent: apiMocks.getCurrent,
    },
    vscodeApi: {
      ...original.vscodeApi,
      getLiveProviderSettings: apiMocks.getLiveProviderSettings,
    },
  };
});

vi.mock("@/components/common/FullScreenPanel", () => ({
  FullScreenPanel: ({
    isOpen,
    children,
    footer,
  }: {
    isOpen: boolean;
    children: React.ReactNode;
    footer?: React.ReactNode;
  }) =>
    isOpen ? (
      <div>
        <div>{children}</div>
        <div>{footer}</div>
      </div>
    ) : null,
}));

import { EditProviderDialog } from "@/components/providers/EditProviderDialog";

const OPENCODE_GO: Provider = {
  id: "8143d143-91e7-4962-b3d1-e788e3e4b2c1",
  name: "OpenCode Go",
  category: "third_party",
  settingsConfig: {
    auth: { OPENAI_API_KEY: "sk-test" },
    config: `model_provider = "custom"
model = "glm-5.2"

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
      ],
    },
  },
  meta: {
    commonConfigEnabled: false,
    endpointAutoSelect: true,
    apiFormat: "openai_responses",
  },
};

describe("EditProviderDialog 真实表单全链路：模型映射保存", () => {
  it("添加一行映射后点保存，提交的 provider.settingsConfig 含新行", async () => {
    setSettings({ commonConfigConfirmed: true });
    // OpenCode Go 不是当前供应商：不读 live，直接用 DB 配置
    apiMocks.getCurrent.mockResolvedValue("some-other-provider");

    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={queryClient}>
        <EditProviderDialog
          open
          provider={OPENCODE_GO}
          onOpenChange={vi.fn()}
          onSubmit={onSubmit}
          appId="codex"
        />
      </QueryClientProvider>,
    );

    await waitFor(() => {
      expect(
        screen.getAllByPlaceholderText("例如: deepseek-v4-flash"),
      ).toHaveLength(2);
    });

    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));

    const modelInputs = await screen.findAllByPlaceholderText(
      "例如: deepseek-v4-flash",
    );
    const displayInputs = screen.getAllByPlaceholderText(
      "例如: DeepSeek V4 Flash",
    );
    expect(modelInputs).toHaveLength(3);

    fireEvent.change(displayInputs[2], { target: { value: "GPT-5.2" } });
    fireEvent.change(modelInputs[2], { target: { value: "qwen3.5-max" } });

    fireEvent.click(screen.getByRole("button", { name: "common.save" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const submitted = onSubmit.mock.calls[0][0].provider as Provider;
    const catalog = (
      submitted.settingsConfig as {
        modelCatalog?: { models: Array<{ model: string }> };
      }
    ).modelCatalog;
    expect(catalog?.models.map((m) => m.model)).toEqual([
      "glm-5.2",
      "glm-5.1",
      "qwen3.5-max",
    ]);
  });
});
