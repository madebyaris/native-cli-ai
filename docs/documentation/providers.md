# Providers

nca supports four LLM provider backends. You can switch between them at any time via config, environment variables, or the interactive `/connect` and `/provider` commands.

## Supported Providers

| Provider | Default Model | API Style | Description |
|----------|---------------|-----------|-------------|
| **MiniMax** | `MiniMax-M2.7` | Anthropic-compatible | Primary provider. Uses the MiniMax Anthropic-compatible endpoint. |
| **Anthropic** | `claude-3-7-sonnet-latest` | Native Anthropic | Direct Anthropic API for Claude models. |
| **OpenAI** | `gpt-4o-mini` | OpenAI Chat | Standard OpenAI chat completions API. |
| **OpenRouter** | `openai/gpt-4o-mini` | OpenAI-compatible | Aggregator providing access to 100+ models from multiple providers. |

## MiniMax (Default)

MiniMax is nca's primary provider, using an Anthropic-compatible API endpoint.

### Setup

```bash
export MINIMAX_API_KEY="your-minimax-api-key"
```

Or in config:

```toml
# ~/.nca/config.toml
[provider]
default = "minimax"

[provider.minimax]
api_key = "your-key"
base_url = "https://api.minimax.io/anthropic"
model = "MiniMax-M2.7"
temperature = 0.7
```

### Features

- Anthropic-compatible protocol (`/v1/messages` endpoint)
- Extended thinking support
- Native vision/image processing
- Streaming responses

## Anthropic

Direct access to Claude models via the native Anthropic API.

### Setup

```bash
export ANTHROPIC_API_KEY="your-anthropic-key"
```

```toml
[provider]
default = "anthropic"

[provider.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
model = "claude-3-7-sonnet-latest"
temperature = 1.0
```

## OpenAI

Standard OpenAI chat completions API.

### Setup

```bash
export OPENAI_API_KEY="your-openai-key"
```

```toml
[provider]
default = "openai"

[provider.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com"
model = "gpt-4o-mini"
temperature = 0.7
```

### OpenAI-Compatible Endpoints

You can point the OpenAI provider at any OpenAI-compatible API by changing `base_url`:

```toml
[provider.openai]
base_url = "https://my-local-llm:8080"
model = "local-model"
```

## OpenRouter

Access to hundreds of models from multiple providers through a single API key.

### Setup

```bash
export OPENROUTER_API_KEY="your-openrouter-key"
```

```toml
[provider]
default = "openrouter"

[provider.openrouter]
api_key_env = "OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api"
model = "openai/gpt-4o-mini"
temperature = 0.7
site_url = "https://my-app.com"    # Optional
app_name = "my-app"                # Optional
```

### Model Format

OpenRouter uses `provider/model` naming:

```
openai/gpt-4o
anthropic/claude-3-7-sonnet
google/gemini-2.0-flash
meta-llama/llama-3.1-70b-instruct
```

## Switching Providers

### Via CLI Flag

```bash
nca --model "claude-3-7-sonnet-latest"
```

### Via Environment Variable

```bash
NCA_DEFAULT_PROVIDER=anthropic nca
NCA_MODEL=gpt-4o nca
```

### Via Interactive Commands

```
/connect           # Opens provider picker UI
/provider openai   # Switch default provider
/model gpt-4o      # Switch model
/models            # Browse available models
```

### Via Config

```toml
[provider]
default = "anthropic"
```

## Model Aliases

nca ships with built-in model aliases for quick switching:

| Alias | Resolves To |
|-------|-------------|
| `default` | `MiniMax-M2.7` |
| `minimax` | `MiniMax-M2.7` |
| `m2.7` | `MiniMax-M2.7` |
| `coding` | `MiniMax-M2.7` |
| `reasoning` | `MiniMax-M2.7` |
| `openai` | `gpt-4o-mini` |
| `gpt4o` | `gpt-4o` |
| `claude` | `claude-3-7-sonnet-latest` |
| `openrouter` | `openai/gpt-4o-mini` |

Add custom aliases in config:

```toml
[model.aliases]
fast = "gpt-4o-mini"
smart = "claude-3-7-sonnet-latest"
local = "ollama/llama3"
```

Use aliases anywhere a model name is expected:

```bash
nca --model fast
```

```
/model smart
```

## API Key Resolution

For each provider, the API key is resolved in this order:

1. **`api_key`** field in config (not recommended for security)
2. **`api_key_env`** — read from the named environment variable (default and recommended)

The default environment variable names are:

| Provider | Variable |
|----------|----------|
| MiniMax | `MINIMAX_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |

You can change the environment variable name via `api_key_env` in config.

## Extended Thinking

Some models support extended thinking (chain-of-thought reasoning). Enable it with:

```bash
nca -t                        # Enable with default budget (5120 tokens)
nca -t --thinking-budget 10000  # Custom budget
```

Or in config:

```toml
[model]
enable_thinking = true
thinking_budget = 10000
```

Toggle visibility in an interactive session:

```
/thinking
```

## Context Window Management

nca auto-detects context window sizes by querying the provider's model API. This enables automatic context compaction when the conversation gets too long.

```toml
[memory.context]
auto_detect_context_window = true
query_provider_models_api = true
max_retained_messages = 50
auto_summarize_threshold = 75     # Trigger at 75% of context window
enable_auto_summarize = true
```

Disable provider API queries if needed:

```bash
export NCA_SKIP_CONTEXT_API=1
```
