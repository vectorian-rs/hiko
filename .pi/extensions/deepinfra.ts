import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const OPENAI_COMPAT = {
	supportsStore: false,
	supportsDeveloperRole: false,
	maxTokensField: "max_tokens" as const,
};

const DEEPINFRA_REASONING_LEVELS = {
	off: "none",
	minimal: "low",
	low: "low",
	medium: "medium",
	high: "high",
	xhigh: "xhigh",
};

const OPENAI_REASONING_COMPAT = {
	...OPENAI_COMPAT,
	supportsReasoningEffort: true,
};

export default function (pi: ExtensionAPI) {
	pi.registerProvider("deepinfra", {
		name: "DeepInfra",
		baseUrl: "https://api.deepinfra.com/v1/openai",
		apiKey: "DEEPINFRA_TOKEN",
		api: "openai-completions",
		models: [
			{
				id: "nvidia/Nemotron-3-Nano-Omni-30B-A3B-Reasoning",
				name: "NVIDIA Nemotron 3 Nano Omni 30B A3B Reasoning",
				reasoning: true,
				thinkingLevelMap: DEEPINFRA_REASONING_LEVELS,
				input: ["text", "image"],
				cost: { input: 0.2, output: 0.8, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 262144,
				maxTokens: 16384,
				compat: OPENAI_REASONING_COMPAT,
			},
			{
				id: "moonshotai/Kimi-K2.6",
				name: "Kimi K2.6",
				reasoning: true,
				thinkingLevelMap: DEEPINFRA_REASONING_LEVELS,
				input: ["text", "image"],
				cost: { input: 0.75, output: 3.5, cacheRead: 0.15, cacheWrite: 0 },
				contextWindow: 262144,
				maxTokens: 16384,
				compat: OPENAI_REASONING_COMPAT,
			},
			{
				id: "Qwen/Qwen3.6-35B-A3B",
				name: "Qwen3.6 35B A3B",
				reasoning: true,
				thinkingLevelMap: DEEPINFRA_REASONING_LEVELS,
				input: ["text", "image"],
				cost: { input: 0.15, output: 0.95, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 262144,
				maxTokens: 16384,
				compat: OPENAI_REASONING_COMPAT,
			},
			{
				id: "Qwen/Qwen3-Max-Thinking",
				name: "Qwen3 Max Thinking",
				reasoning: true,
				thinkingLevelMap: DEEPINFRA_REASONING_LEVELS,
				input: ["text", "image"],
				cost: { input: 0.5, output: 2.5, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 131072,
				maxTokens: 16384,
				compat: OPENAI_REASONING_COMPAT,
			},
			{
				id: "Qwen/Qwen3.7-Max",
				name: "Qwen3.7 Max",
				input: ["text", "image"],
				cost: { input: 0.5, output: 2.5, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 131072,
				maxTokens: 16384,
				compat: OPENAI_COMPAT,
			},
			{
				id: "Qwen/Qwen3-Coder-480B-A35B-Instruct-Turbo",
				name: "Qwen3 Coder 480B A35B Instruct Turbo",
				input: ["text", "image"],
				cost: { input: 0.4, output: 1.5, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 131072,
				maxTokens: 16384,
				compat: OPENAI_COMPAT,
			},
			{
				id: "nvidia/NVIDIA-Nemotron-3-Super-120B-A12B",
				name: "NVIDIA Nemotron 3 Super 120B A12B",
				reasoning: true,
				thinkingLevelMap: DEEPINFRA_REASONING_LEVELS,
				input: ["text"],
				cost: { input: 0.1, output: 0.5, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 262144,
				maxTokens: 16384,
				compat: OPENAI_REASONING_COMPAT,
			},
		],
	});

	pi.on("message_end", (event, ctx) => {
		const message = event.message;
		if (message.role !== "assistant") return;
		if (message.stopReason !== "error") return;
		if (message.provider !== "deepinfra" && ctx.model?.provider !== "deepinfra") return;

		const errorMessage = message.errorMessage ?? "";
		if (errorMessage.includes("context_length_exceeded")) return;
		if (!/(context (?:length|size|window)|maximum context|exceeded[^\n]*context|too many tokens)/i.test(errorMessage)) {
			return;
		}

		return {
			message: {
				...message,
				errorMessage: `context_length_exceeded: ${errorMessage}`,
			},
		};
	});
}
