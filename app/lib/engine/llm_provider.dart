// Model backends for the on-device engine: turn the room's situation into
// tool calls and prose through the same APIs the desktop providers use.
// Pure request/response builders are separated from transport so they are
// testable without a network. Two wire formats are spoken: OpenAI-compatible
// chat completions (DeepSeek, most local and hosted endpoints) and the
// Anthropic Messages API.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';
import 'dart:io';

import 'engine_core.dart';
import 'runner.dart';

/// Which chat-completions dialect to speak.
enum LlmKind { openAiCompatible, anthropic }

class LlmConfig {
  final LlmKind kind;
  final String apiKey;
  final String model;
  final String baseUrl; // e.g. https://api.deepseek.com/v1 or https://api.anthropic.com
  final String system;
  LlmConfig({
    required this.kind,
    required this.apiKey,
    required this.model,
    this.baseUrl = '',
    this.system = '',
  });
}

/// A POST that returns (status, body). Real calls go through [httpPost];
/// tests inject fakes.
typedef Post = Future<(int, String)> Function(
    String url, Map<String, Object?> body, Map<String, String> headers);

/// The default transport: dart:io HttpClient with timeouts.
Future<(int, String)> httpPost(
    String url, Map<String, Object?> body, Map<String, String> headers) async {
  final client = HttpClient()
    ..connectionTimeout = const Duration(seconds: 30);
  try {
    final request = await client.postUrl(Uri.parse(url));
    headers.forEach((k, v) => request.headers.set(k, v));
    request.headers.contentType = ContentType.json;
    request.write(jsonEncode(body));
    final response = await request.close().timeout(const Duration(seconds: 120));
    final text = await response.transform(utf8.decoder).join();
    return (response.statusCode, text);
  } finally {
    client.close();
  }
}

/// The room's tool surface, in the provider-neutral shape both dialects use.
class RoomTool {
  final String name;
  final String description;
  final Map<String, Object?> inputSchema;
  const RoomTool(this.name, this.description, this.inputSchema);
}

/// The tools every bot may see; the engine still refuses what its role
/// cannot do, but the model is steered by what it is offered.
List<RoomTool> engineTools() {
  Map<String, Object?> s(String d) => {'type': 'string', 'description': d};
  Map<String, Object?> obj(Map<String, Object?> props, List<String> required) =>
      {'type': 'object', 'properties': props, 'required': required};
  Map<String, Object?> ts(Map<String, Object?> extra, List<String> extraRequired) =>
      obj(<String, Object?>{'task': s('The task id.')}..addAll(extra), ['task', ...extraRequired]);

  return [
    RoomTool('room_status', 'See who is in the room and the state of every task.', obj({}, [])),
    RoomTool(
        'list_tasks',
        'List tasks. Pass filter = claimable for open work you can take.',
        obj(<String, Object?>{
          'filter': s('all | mine | claimable | in_review'),
        }, [])),
    RoomTool('claim_task', 'Take an open (ready) task.', ts({}, [])),
    RoomTool('start_task', 'Start working a claimed task.', ts({}, [])),
    RoomTool(
        'add_artifact',
        'Record work done on a task. The artifact is the deliverable text.',
        ts(<String, Object?>{'body': s('The work, as text.')}, ['body'])),
    RoomTool('submit_for_review', 'Send completed work to an inspector.', ts({}, [])),
    RoomTool('release_task', 'Put a claimed task back on the board.', ts({}, [])),
    RoomTool(
        'record_review',
        'Inspect work in review. Omit findings to approve; a major finding sends it back; a critical one fails it.',
        ts(<String, Object?>{
          'summary': s('Overall assessment.'),
          'findings': {
            'type': 'array',
            'items': {'type': 'object'},
            'description': 'Problems found; empty approves.',
          },
        }, ['summary'])),
    RoomTool('submit_task', 'Submit an approved task as finished (submitters only).', ts({}, [])),
    RoomTool(
        'offer_handoff',
        'Ask another bot to take over a task you hold.',
        ts(<String, Object?>{
          'to_agent': s('Handle or id of the bot.'),
          'reason': s('Why.'),
        }, [])),
    RoomTool(
        'post_message',
        'Say something in the room chat.',
        obj(<String, Object?>{'body': s('What to say.')}, ['body'])),
  ];
}

// ---------------------------------------------------------------- builders

/// OpenAI-compatible chat body.
Map<String, Object?> openAiBody({
  required String system,
  required String context,
  required List<RoomTool> tools,
}) {
  final toolSpecs = tools
      .map((t) => {
            'type': 'function',
            'function': {
              'name': t.name,
              'description': t.description,
              'parameters': t.inputSchema,
            }
          })
      .toList();
  return {
    'model': 'REPLACED', // filled by the provider
    'messages': [
      {'role': 'system', 'content': system},
      {'role': 'user', 'content': context},
    ],
    'tools': toolSpecs,
    'tool_choice': 'auto',
  };
}

/// Parse an OpenAI-compatible response into a bot turn.
BotTurn parseOpenAi(String body) {
  final root = jsonDecode(body) as Map<String, Object?>;
  final choice = (root['choices'] as List).first as Map<String, Object?>;
  final message = choice['message'] as Map<String, Object?>;
  final calls = <ToolCall>[];
  final rawCalls = message['tool_calls'];
  if (rawCalls is List) {
    for (final raw in rawCalls) {
      final fn = (raw as Map<String, Object?>)['function'] as Map<String, Object?>;
      final name = fn['name'] as String;
      final args = jsonDecode(fn['arguments'] as String) as Map<String, Object?>;
      calls.add(ToolCall(name, args));
    }
  }
  final content = message['content'];
  return BotTurn(
    tools: calls,
    text: content is String && content.trim().isNotEmpty ? content.trim() : null,
  );
}

/// Anthropic Messages body (tools use the native format).
Map<String, Object?> anthropicBody({
  required String system,
  required String context,
  required List<RoomTool> tools,
  int maxTokens = 2048,
}) {
  return {
    'model': 'REPLACED',
    'max_tokens': maxTokens,
    'system': system,
    'messages': [
      {'role': 'user', 'content': context},
    ],
    'tools': tools
        .map((t) => {
              'name': t.name,
              'description': t.description,
              'input_schema': t.inputSchema,
            })
        .toList(),
  };
}

/// Parse an Anthropic Messages response.
BotTurn parseAnthropic(String body) {
  final root = jsonDecode(body) as Map<String, Object?>;
  final blocks = (root['content'] as List).cast<Map<String, Object?>>();
  final text = <String>[];
  final calls = <ToolCall>[];
  for (final block in blocks) {
    final type = block['type'];
    if (type == 'text') {
      final t = block['text'];
      if (t is String && t.trim().isNotEmpty) text.add(t.trim());
    } else if (type == 'tool_use') {
      calls.add(ToolCall(
        block['name'] as String,
        (block['input'] as Map?)?.cast<String, Object?>() ?? const {},
      ));
    }
  }
  return BotTurn(tools: calls, text: text.isNotEmpty ? text.join('\n') : null);
}

// ------------------------------------------------------------------ provider

/// A bot whose backend is a chat-completions API.
class LlmBot implements BotProvider {
  final LlmConfig config;
  final Post post;
  final List<RoomTool> tools;
  LlmBot(this.config, {Post? post, List<RoomTool>? tools})
      : post = post ?? httpPost,
        tools = tools ?? engineTools();

  @override
  String get name => config.model;

  Uri _uri() {
    final base = config.baseUrl.isEmpty ? _defaultBase(config.kind) : config.baseUrl;
    final path = config.kind == LlmKind.anthropic ? '/v1/messages' : '/chat/completions';
    return Uri.parse(base.endsWith('/') ? base + path : base + path);
  }

  Map<String, String> _headers() {
    if (config.kind == LlmKind.anthropic) {
      return {
        'x-api-key': config.apiKey,
        'anthropic-version': '2023-06-01',
      };
    }
    return {'Authorization': 'Bearer ' + config.apiKey};
  }

  @override
  Future<BotTurn> act({required String situation}) async {
    final body = config.kind == LlmKind.anthropic
        ? anthropicBody(system: config.system, context: situation, tools: tools)
        : openAiBody(system: config.system, context: situation, tools: tools);
    body['model'] = config.model;
    final (status, raw) =
        await post(_uri().toString(), body, _headers());
    if (status >= 300) {
      throw EngineError('model API returned HTTP ' +
          status.toString() +
          ': ' +
          (raw.length > 300 ? raw.substring(0, 300) : raw));
    }
    return config.kind == LlmKind.anthropic
        ? parseAnthropic(raw)
        : parseOpenAi(raw);
  }
}

String _defaultBase(LlmKind kind) =>
    kind == LlmKind.anthropic ? 'https://api.anthropic.com' : 'https://api.deepseek.com/v1';
