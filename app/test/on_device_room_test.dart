// The full standalone stack, end to end: wire-format model backends drive
// the on-device runner through a complete room - coordinator seeds, writer
// claims and works (three tool calls in one response), inspector approves,
// writer submits. One bot speaks OpenAI-compatible, one speaks Anthropic, so
// both dialects are exercised through the same engine.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:gptbots_app/engine/engine_core.dart';
import 'package:gptbots_app/engine/llm_provider.dart';
import 'package:gptbots_app/engine/runner.dart';

/// A fake model that answers from the situation text, in the dialect its
/// headers demand, mirroring what a real endpoint would return.
Future<(int, String)> fakeModel(
    String url, Map<String, Object?> body, Map<String, String> headers) async {
  final model = body['model'] as String;
  final context = ((body['messages'] as List).last as Map)['content'] as String;
  bool boardState(String needle) => context.contains(needle);

  String openai(String content, List<Map<String, Object?>> calls) {
    final toolCalls = calls.map((c) => {
          'id': 'call_' + (c['name'] as String),
          'type': 'function',
          'function': {
            'name': c['name'],
            'arguments': jsonEncode(c['args']),
          },
        }).toList();
    return jsonEncode({
      'choices': [
        {
          'message': {
            'role': 'assistant',
            'content': content.isEmpty ? null : content,
            if (toolCalls.isNotEmpty) 'tool_calls': toolCalls,
          }
        }
      ]
    });
  }

  String anthropic(List<Map<String, Object?>> uses, {String? text}) {
    final blocks = <Map<String, Object?>>[
      ...uses.map((u) => {
            'type': 'tool_use',
            'id': 'toolu_' + (u['name'] as String),
            'name': u['name'],
            'input': u['args'],
          }),
      if (text != null && text.isNotEmpty) {'type': 'text', 'text': text},
    ];
    return jsonEncode({'content': blocks});
  }

  if (model == 'lead-model') {
    if (!boardState('task_')) {
      return (200, openai('', [
        {
          'name': 'create_task',
          'args': {
            'title': 'write the notes',
            'brief': 'draft, review, ship',
            'required_role': 'writer',
          }
        }
      ]));
    }
    return (200, openai('', [])); // nothing left for the coordinator
  }

  if (model == 'writer-model') {
    // Writer speaks Anthropic. On a ready task it does the whole job in one
    // response (parallel tool_use); on an approved task it submits.
    final ready = boardState('ready task_');
    final approved = boardState('approved task_');
    final id = RegExp('task_[0-9a-f]+').firstMatch(context)!.group(0)!;
    if (approved) {
      return (200, anthropic([
        {'name': 'submit_task', 'args': {'task': id}}
      ], text: 'submitted the notes'));
    }
    if (ready) {
      return (200, anthropic([
        {'name': 'claim_task', 'args': {'task': id}},
        {'name': 'start_task', 'args': {'task': id}},
        {
          'name': 'add_artifact',
          'args': {'task': id, 'body': 'release notes draft'}
        },
        {'name': 'submit_for_review', 'args': {'task': id}},
      ]));
    }
    return (200, anthropic([], text: ''));
  }

  // qa-model speaks OpenAI.
  if (boardState('inReview task_')) {
    final id = RegExp('task_[0-9a-f]+').firstMatch(context)!.group(0)!;
    return (200, openai('', [
      {
        'name': 'record_review',
        'args': {'task': id, 'summary': 'covers the changes'}
      }
    ]));
  }
  return (200, openai('', []));
}

void main() {
  test('a full on-device room runs with real wire-format model backends',
      () async {
    final engine = Engine(name: 'notes', goal: 'write and ship the notes');
    final runner = Runner(engine);

    runner.seat(
      'lead',
      const ['coordinator'],
      LlmBot(LlmConfig(
        kind: LlmKind.openAiCompatible,
        apiKey: 'k',
        model: 'lead-model',
        baseUrl: 'https://fake/v1',
        system: 'You are the coordinator. Seed the board when it is empty.',
      ), post: fakeModel),
      mayCreate: true,
      maySubmit: true,
    );
    runner.seat(
      'writer',
      const ['writer'],
      LlmBot(LlmConfig(
        kind: LlmKind.anthropic,
        apiKey: 'k',
        model: 'writer-model',
        system: 'You are a writer on the board.',
      ), post: fakeModel),
      maySubmit: true,
    );
    runner.seat(
      'qa',
      const ['reviewer'],
      LlmBot(LlmConfig(
        kind: LlmKind.openAiCompatible,
        apiKey: 'k',
        model: 'qa-model',
        baseUrl: 'https://fake/v1',
        system: 'You inspect work in review.',
      ), post: fakeModel),
      mayInspect: true,
    );

    final outcome = await runner.run();
    expect(outcome, RunOutcome.completed);

    final task = engine.tasks.values.single;
    expect(task.state, TaskState.submitted);
    expect(task.artifacts, hasLength(1));
    // Both the writer (anthropic) and whoever posted said something.
    expect(engine.messages, isNotEmpty);
    // The room transcript is the event-sourced story.
    expect(engine.log.any((l) => l.contains('approved by @qa')), isTrue);
  });
}
