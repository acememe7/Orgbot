// Wire-format tests for the on-device model backends: request builders and
// response parsers are pure, so they are locked down here without a network,
// and a fake transport proves a provider round-trips into a bot turn.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter_test/flutter_test.dart';
import 'package:gptbots_app/engine/engine_core.dart';
import 'package:gptbots_app/engine/llm_provider.dart';

const openAiToolCall = r'''{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"claim_task","arguments":"{\"task\":\"task_1\"}"}}]}}]}''';
const openAiText = r'''{"choices":[{"message":{"role":"assistant","content":"on it"}}]}''';
const anthropicToolUse = r'''{"content":[{"type":"tool_use","id":"toolu_1","name":"record_review","input":{"task":"task_9","summary":"ok"}}],"stop_reason":"tool_use"}''';
const anthropicText = r'''{"content":[{"type":"text","text":"approved"}]}''';

void main() {
  test('the openai-compatible body carries the system, context and tools', () {
    final body = openAiBody(
      system: 'you are a writer',
      context: 'Board: ready task_1 - write it',
      tools: engineTools(),
    );
    final msgs = body['messages']! as List;
    expect((msgs[0] as Map)['role'], 'system');
    expect((msgs[1] as Map)['role'], 'user');
    final tools = body['tools']! as List;
    expect(tools.length, engineTools().length);
    final claim = tools.firstWhere((t) => ((t as Map)['function'] as Map)['name'] == 'claim_task');
    expect((claim['function'] as Map)['description'], contains('Take'));
  });

  test('an openai tool-call response becomes a bot turn', () {
    final turn = parseOpenAi(openAiToolCall);
    expect(turn.tools.single.name, 'claim_task');
    expect(turn.tools.single.args['task'], 'task_1');
    expect(turn.text, isNull);
  });

  test('an openai text response becomes prose', () {
    final turn = parseOpenAi(openAiText);
    expect(turn.tools, isEmpty);
    expect(turn.text, 'on it');
  });

  test('the anthropic body uses the native tools format', () {
    final body = anthropicBody(
      system: 'be an inspector',
      context: 'Board: inReview task_9 - check me',
      tools: engineTools(),
    );
    expect(body['max_tokens'], 2048);
    final tools = body['tools']! as List;
    final review = tools.firstWhere((t) => (t as Map)['name'] == 'record_review');
    expect(review['input_schema'], isA<Map<Object?, Object?>>());
  });

  test('an anthropic tool_use response becomes a bot turn', () {
    final turn = parseAnthropic(anthropicToolUse);
    expect(turn.tools.single.name, 'record_review');
    expect(turn.tools.single.args['summary'], 'ok');
  });

  test('an anthropic text block becomes prose', () {
    final turn = parseAnthropic(anthropicText);
    expect(turn.text, 'approved');
  });

  Future<(int, String)> fakeOpenAi(
      String url, Map<String, Object?> body, Map<String, String> headers) async {
    expect(headers['Authorization'], 'Bearer sk-test');
    expect(url, 'https://api.deepseek.com/v1/chat/completions');
    expect(body['model'], 'deepseek-chat');
    return (200, openAiToolCall);
  }

  test('an LlmBot round-trips through a fake transport', () async {
    final provider = LlmBot(
      LlmConfig(
        kind: LlmKind.openAiCompatible,
        apiKey: 'sk-test',
        model: 'deepseek-chat',
        baseUrl: 'https://api.deepseek.com/v1',
        system: 'you work the board',
      ),
      post: fakeOpenAi,
    );
    final turn = await provider.act(situation: 'Board: ready task_1 - write');
    expect(turn.tools.single.name, 'claim_task');
    expect(turn.tools.single.args['task'], 'task_1');
  });

  Future<(int, String)> fakeAnthropic(
      String url, Map<String, Object?> body, Map<String, String> headers) async {
    expect(headers['x-api-key'], 'ant-key');
    expect(headers['anthropic-version'], '2023-06-01');
    return (200, anthropicText);
  }

  test('an anthropic LlmBot sends the api key header', () async {
    final provider = LlmBot(
      LlmConfig(
        kind: LlmKind.anthropic,
        apiKey: 'ant-key',
        model: 'claude-sonnet-5',
      ),
      post: fakeAnthropic,
    );
    final turn = await provider.act(situation: 'Board: empty');
    expect(turn.text, 'approved');
  });

  Future<(int, String)> failingPost(
          String url, Map<String, Object?> body, Map<String, String> headers) async =>
      (401, 'bad key');

  test('http errors surface as engine errors, not crashes', () async {
    final provider = LlmBot(
      LlmConfig(
        kind: LlmKind.openAiCompatible,
        apiKey: 'bad',
        model: 'x',
        baseUrl: 'https://example.com/v1',
      ),
      post: failingPost,
    );
    await expectLater(provider.act(situation: 'x'), throwsA(isA<EngineError>()));
  });
}
