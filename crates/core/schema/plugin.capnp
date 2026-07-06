# nca Plugin Protocol — Cap'n Proto wire contract
#
# Direction-separated unions with shared Envelope wrapper. Each message carries
# a `requestId` (envelope-level `id`) for concurrent multiplexing.
#
# Transport: self-delimiting Cap'n Proto streaming frames over stdin/stdout.
# stderr is inherited for plugin diagnostics/logs.

@0xab8c4e02f5a1d3b7;

# ═══════════════════════════════════════════════════════════════════════════
# Versioning
# ═══════════════════════════════════════════════════════════════════════════

struct ProtocolVersion {
    major @0 :UInt16;
    minor @1 :UInt16;
}

# ═══════════════════════════════════════════════════════════════════════════
# Tool parameter model (full Cap'n Proto structs — D4)
# ═══════════════════════════════════════════════════════════════════════════

enum ParamType {
    string  @0;
    number  @1;
    integer @2;
    boolean @3;
    array   @4;
    object  @5;
    null    @6;
}

struct ToolParameter {
    name        @0 :Text;
    description @1 :Text;
    type        @2 :ParamType;
    required    @3 :Bool = false;
    enumValues  @4 :List(Text);          # choices for enum-style params
    children    @5 :List(ToolParameter);  # nested fields for object type
}

struct ToolDeclaration {
    name        @0 :Text;
    description @1 :Text;
    parameters  @2 :List(ToolParameter);
}

struct Capabilities {
    tools    @0 :List(ToolDeclaration);
    commands @1 :List(Text);
}

# ═══════════════════════════════════════════════════════════════════════════
# Handshake
# ═══════════════════════════════════════════════════════════════════════════

# Plugin → Host: sent immediately after process spawn.
struct Hello {
    name         @0 :Text;
    version      @1 :Text;
    protocol     @2 :ProtocolVersion;
    capabilities @3 :Capabilities;
}

# Host → Plugin: sent after validating Hello.
struct Config {
    workspaceRoot  @0 :Text;
    sessionId      @1 :Text;
    permissionMode @2 :Text;
}

# ═══════════════════════════════════════════════════════════════════════════
# Typed key-value pair for tool arguments
# ═══════════════════════════════════════════════════════════════════════════

enum ValueType {
    string  @0;
    number  @1;
    integer @2;
    boolean @3;
    json    @4;   # nested objects/arrays as JSON text
}

struct KV {
    key    @0 :Text;
    type   @1 :ValueType;
    str    @2 :Text;
    num    @3 :Float64;
    int    @4 :Int64;
    bool   @5 :Bool;
}

# ═══════════════════════════════════════════════════════════════════════════
# Tool execution (Host → Plugin: request, Plugin → Host: result)
# ═══════════════════════════════════════════════════════════════════════════

struct ExecuteToolRequest {
    toolId @0 :Text;
    args   @1 :List(KV);
}

struct ExecuteToolResult {
    success   @0 :Bool;
    output    @1 :Text;
    error     @2 :Text;
    metadata  @3 :Text;   # JSON-encoded metadata
}

# ═══════════════════════════════════════════════════════════════════════════
# Transformation hooks (sequential accumulation)
# ═══════════════════════════════════════════════════════════════════════════

struct SystemPromptRequest {
    workspaceRoot @0 :Text;
}
struct SystemPromptResult {
    text @0 :Text;   # empty = no contribution
}

struct UserPromptRequest {
    prompt @0 :Text;
}
struct UserPromptResult {
    message @0 :Text;
}

struct ChatParamsRequest {
    temperature     @0 :Float64;
    topP            @1 :Float64;
    topK            @2 :Int32;
    maxOutputTokens @3 :Int32;
    optionsJson     @4 :Text;
}
struct ChatParamsResult {
    temperature     @0 :Float64;
    topP            @1 :Float64;
    topK            @2 :Int32;
    maxOutputTokens @3 :Int32;
    optionsJson     @4 :Text;
}

struct ChatMessagesTransformRequest {
    messagesJson @0 :Text;
}
struct ChatMessagesTransformResult {
    messagesJson @0 :Text;
}

struct ShellEnvRequest {
    cwd       @0 :Text;
    sessionId @1 :Text;
}
struct ShellEnvResult {
    envJson @0 :Text;   # JSON object of env vars
}

struct ToolDefinitionRequest {
    toolId         @0 :Text;
    description    @1 :Text;
    parametersJson @2 :Text;
}
struct ToolDefinitionResult {
    description    @0 :Text;
    parametersJson @1 :Text;
}

# ═══════════════════════════════════════════════════════════════════════════
# Decision hooks (first-responder wins)
# ═══════════════════════════════════════════════════════════════════════════

enum PermissionVerdict {
    pass  @0;   # no opinion, ask next plugin
    allow @1;
    deny  @2;
}

struct PermissionAskRequest {
    tool      @0 :Text;
    inputJson @1 :Text;
}
struct PermissionAskResult {
    verdict @0 :PermissionVerdict;
}

# ═══════════════════════════════════════════════════════════════════════════
# Interception hooks (pre/post)
# ═══════════════════════════════════════════════════════════════════════════

struct ToolExecuteBeforeRequest {
    tool     @0 :Text;
    callId   @1 :Text;
    argsJson @2 :Text;
}
struct ToolExecuteBeforeResult {
    argsJson @0 :Text;
}

struct ToolExecuteAfterRequest {
    tool     @0 :Text;
    callId   @1 :Text;
    argsJson @2 :Text;
    title    @3 :Text;
    output   @4 :Text;
}
struct ToolExecuteAfterResult {
    title        @0 :Text;
    output       @1 :Text;
    metadataJson @2 :Text;
}

struct CommandExecuteBeforeRequest {
    command   @0 :Text;
    sessionId @1 :Text;
    arguments @2 :Text;
}
struct CommandExecuteBeforeResult {
    partsJson @0 :Text;
}

# ═══════════════════════════════════════════════════════════════════════════
# Infrastructure
# ═══════════════════════════════════════════════════════════════════════════

struct EventNotification {
    eventJson @0 :Text;
}

struct ErrorResponse {
    message @0 :Text;
}

# ═══════════════════════════════════════════════════════════════════════════
# Host callbacks (Plugin → Host: request, Host → Plugin: response)
# ═══════════════════════════════════════════════════════════════════════════

struct ReadFileCallback {
    path @0 :Text;
}
struct ReadFileResponse {
    success @0 :Bool;
    content @1 :Text;
    error   @2 :Text;
}

struct ListDirectoryCallback {
    path @0 :Text;
}
struct ListDirectoryResponse {
    success     @0 :Bool;
    entriesJson @1 :Text;
    error       @2 :Text;
}

struct SearchCodeCallback {
    pattern @0 :Text;
}
struct SearchCodeResponse {
    success     @0 :Bool;
    resultsJson @1 :Text;
    error       @2 :Text;
}

struct GetWorkspaceRootCallback {}
struct GetWorkspaceRootResponse {
    path @0 :Text;
}

enum LogLevel {
    trace @0;
    debug @1;
    info  @2;
    warn  @3;
    error @4;
}
struct LogCallback {
    level   @0 :LogLevel;
    message @1 :Text;
}
struct LogResponse {
    ok @0 :Bool;
}

# ═══════════════════════════════════════════════════════════════════════════
# Top-level message envelope
# ═══════════════════════════════════════════════════════════════════════════

struct PluginMessage {
    id   @0 :Text;   # request ID for multiplexing
    body @1 :Body;
}

struct Body {
    union {
        # ── Handshake ───────────────────────────────────────────────────
        hello                 @0  :Hello;
        config                @1  :Config;
        shutdown              @2  :Void;
        refreshCapabilities   @3  :Void;

        # ── Host → Plugin requests ──────────────────────────────────────
        executeTool           @4  :ExecuteToolRequest;
        systemPrompt          @5  :SystemPromptRequest;
        userPrompt            @6  :UserPromptRequest;
        chatParams            @7  :ChatParamsRequest;
        chatMessagesTransform @8  :ChatMessagesTransformRequest;
        shellEnv              @9  :ShellEnvRequest;
        toolDefinition        @10 :ToolDefinitionRequest;
        permissionAsk         @11 :PermissionAskRequest;
        toolExecuteBefore     @12 :ToolExecuteBeforeRequest;
        toolExecuteAfter      @13 :ToolExecuteAfterRequest;
        commandExecuteBefore  @14 :CommandExecuteBeforeRequest;
        event                 @15 :EventNotification;

        # ── Plugin → Host results ───────────────────────────────────────
        executeToolResult     @16 :ExecuteToolResult;
        systemPromptResult    @17 :SystemPromptResult;
        userPromptResult      @18 :UserPromptResult;
        chatParamsResult      @19 :ChatParamsResult;
        chatMessagesTransformResult @20 :ChatMessagesTransformResult;
        shellEnvResult        @21 :ShellEnvResult;
        toolDefinitionResult  @22 :ToolDefinitionResult;
        permissionAskResult   @23 :PermissionAskResult;
        toolExecuteBeforeResult @24 :ToolExecuteBeforeResult;
        toolExecuteAfterResult  @25 :ToolExecuteAfterResult;
        commandExecuteBeforeResult @26 :CommandExecuteBeforeResult;
        capabilitiesResult    @27 :Capabilities;

        # ── Plugin → Host callbacks ─────────────────────────────────────
        readFile              @28 :ReadFileCallback;
        listDirectory         @29 :ListDirectoryCallback;
        searchCode            @30 :SearchCodeCallback;
        getWorkspaceRoot      @31 :GetWorkspaceRootCallback;
        log                   @32 :LogCallback;

        # ── Host → Plugin callback responses ────────────────────────────
        readFileResponse      @33 :ReadFileResponse;
        listDirectoryResponse @34 :ListDirectoryResponse;
        searchCodeResponse    @35 :SearchCodeResponse;
        getWorkspaceRootResponse @36 :GetWorkspaceRootResponse;
        logResponse           @37 :LogResponse;

        # ── Error ───────────────────────────────────────────────────────
        error                 @38 :ErrorResponse;
    }
}
