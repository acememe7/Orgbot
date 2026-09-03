// ignore_for_file: prefer_interpolation_to_compose_strings

// Run a room on this device: pick a backend and model per bot, drop in your
// API keys, and watch the board and chat live. No server, no PC needed.
library;

import 'package:flutter/material.dart';

import '../engine/engine_core.dart';
import '../engine/llm_provider.dart';
import '../engine/runner.dart';
import '../state/local_room_controller.dart';

/// Which backend a bot row uses.
enum _Backend { scripted, anthropic, deepseek }

class _BackendChoice {
  final _Backend kind;
  final String model;
  const _BackendChoice(this.kind, this.model);
}

const _choices = [
  _BackendChoice(_Backend.scripted, 'demo (no key)'),
  _BackendChoice(_Backend.anthropic, 'claude-sonnet-5'),
  _BackendChoice(_Backend.deepseek, 'deepseek-chat'),
];

class _Row {
  final String handle;
  final List<String> roles;
  _BackendChoice choice;
  _Row(this.handle, this.roles, this.choice);
}

/// A keyless demo bot that works the board by reading the situation.
class _DemoBot implements BotProvider {
  final String handle;
  _DemoBot(this.handle);
  @override
  String get name => handle;

  String _id(String text, String prefix) {
    final words = text.split(RegExp('[^A-Za-z0-9_]+'));
    for (final w in words) {
      if (w.startsWith(prefix + '_') && w.length > prefix.length + 2) return w;
    }
    return '';
  }

  String _state(String text, String id) {
    final i = text.lastIndexOf('Board:');
    final board = i < 0 ? text : text.substring(i);
    final j = board.lastIndexOf(id);
    if (j < 0) return '';
    final before = board.substring(0, j).trim();
    final words = before.split(RegExp('\\s+'));
    return words.isEmpty ? '' : words.last;
  }

  @override
  Future<BotTurn> act({required String situation}) async {
    final task = _id(situation, 'task');
    if (handle == 'lead') {
      if (task.isEmpty) {
        return const BotTurn(tools: [
          ToolCall('create_task', {
            'title': 'do the job',
            'brief': 'claim, work, review, ship',
            'required_role': 'writer',
          })
        ]);
      }
      return const BotTurn();
    }
    if (task.isEmpty) return const BotTurn();
    final state = _state(situation, task);
    if (handle == 'writer') {
      if (state == 'ready') {
        return BotTurn(tools: [ToolCall('claim_task', {'task': task})]);
      }
      if (state == 'claimed') {
        await Future<void>.delayed(const Duration(milliseconds: 200));
        return BotTurn(tools: [
          ToolCall('start_task', {'task': task}),
          ToolCall('add_artifact', {'task': task, 'body': 'the work'}),
          ToolCall('submit_for_review', {'task': task}),
        ]);
      }
      if (state == 'approved') {
        return BotTurn(
            text: 'shipped it',
            tools: [ToolCall('submit_task', {'task': task})]);
      }
      return const BotTurn();
    }
    if (handle == 'qa' && state == 'inReview') {
      return BotTurn(tools: [
        ToolCall('record_review', {'task': task, 'summary': 'looks good'})
      ]);
    }
    return const BotTurn();
  }
}

/// Build a real model bot from a row's backend and the key the user typed.
BotProvider _realBot(_Row row, String anthropicKey, String openAiKey) {
  switch (row.choice.kind) {
    case _Backend.anthropic:
      return LlmBot(LlmConfig(
        kind: LlmKind.anthropic,
        apiKey: anthropicKey,
        model: row.choice.model,
        system: 'You are ' + row.handle + ' in a room of bots sharing a task board.',
      ));
    case _Backend.deepseek:
      return LlmBot(LlmConfig(
        kind: LlmKind.openAiCompatible,
        apiKey: openAiKey,
        model: row.choice.model,
        baseUrl: 'https://api.deepseek.com/v1',
        system: 'You are ' + row.handle + ' in a room of bots sharing a task board.',
      ));
    case _Backend.scripted:
      return _DemoBot(row.handle);
  }
}

/// The on-device room screen.
class LocalRoomScreen extends StatefulWidget {
  const LocalRoomScreen({super.key});
  @override
  State<LocalRoomScreen> createState() => _LocalRoomScreenState();
}

class _LocalRoomScreenState extends State<LocalRoomScreen> {
  final _goal = TextEditingController(
      text: 'Write and ship the release notes for version 1.3.');
  final _anthropicKey = TextEditingController();
  final _openAiKey = TextEditingController();
  final _rows = [
    _Row('lead', const ['coordinator'], _choices[0]),
    _Row('writer', const ['writer'], _choices[0]),
    _Row('qa', const ['reviewer'], _choices[0]),
  ];
  LocalRoomController? _controller;
  final _logOpen = ValueNotifier<bool>(false);

  @override
  void dispose() {
    _goal.dispose();
    _anthropicKey.dispose();
    _openAiKey.dispose();
    _logOpen.dispose();
    super.dispose();
  }

  void _start() {
    final engine = Engine(name: 'local', goal: _goal.text.trim());
    final runner = Runner(engine);
    final controller = LocalRoomController(engine: engine, runner: runner);
    for (final row in _rows) {
      final key = row.choice.kind == _Backend.anthropic
          ? _anthropicKey.text.trim()
          : _openAiKey.text.trim();
      final needsKey = row.choice.kind != _Backend.scripted;
      if (needsKey && key.isEmpty) {
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(
          content: Text('Add the ' +
              (row.choice.kind == _Backend.anthropic ? 'Anthropic' : 'OpenAI-compatible') +
              ' API key for @' +
              row.handle),
        ));
        return;
      }
      controller.seat(
        row.handle,
        row.roles,
        _realBot(row, _anthropicKey.text.trim(), _openAiKey.text.trim()),
        mayCreate: row.handle == 'lead',
        maySubmit: row.handle != 'qa',
        mayInspect: row.handle == 'qa',
      );
    }
    setState(() => _controller = controller);
    controller.start();
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Run a room on this device'),
        actions: [
          if (controller != null)
            ValueListenableBuilder<bool>(
              valueListenable: _logOpen,
              builder: (context, open, _) => TextButton(
                onPressed: () => _logOpen.value = !open,
                child: Text(open ? 'Hide log' : 'Show log'),
              ),
            ),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          TextField(
            controller: _goal,
            maxLines: 2,
            decoration: const InputDecoration(
              labelText: 'Room goal',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 16),
          Text('Provider keys', style: Theme.of(context).textTheme.titleSmall),
          _keyField(_anthropicKey, 'Anthropic API key', 'sk-ant-...'),
          _keyField(_openAiKey, 'OpenAI-compatible key (DeepSeek/Codex)', 'sk-...'),
          const SizedBox(height: 16),
          Text('Bots', style: Theme.of(context).textTheme.titleSmall),
          for (final row in _rows) _botRow(row),
          const SizedBox(height: 16),
          FilledButton.icon(
            onPressed:
                controller != null && controller.state == LocalRunState.running
                    ? null
                    : _start,
            icon: const Icon(Icons.play_arrow),
            label: const Text('Run room'),
          ),
          if (controller != null) ...[
            const SizedBox(height: 16),
            AnimatedBuilder(
              animation: controller,
              builder: (context, _) {
                final snap = controller.snapshot();
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(children: [
                      Chip(label: Text('phase: ' + snap.phase)),
                      if (controller.outcome != null)
                        Chip(label: Text('outcome: ' + controller.outcome!)),
                      if (controller.error != null)
                        Chip(label: Text('error: ' + controller.error!)),
                      if (controller.state == LocalRunState.running) ...[
                        TextButton(onPressed: controller.pause, child: const Text('Pause')),
                      ] else if (controller.state == LocalRunState.paused) ...[
                        TextButton(onPressed: controller.resume, child: const Text('Resume')),
                      ],
                    ]),
                    Text('Board', style: Theme.of(context).textTheme.titleSmall),
                    for (final t in snap.tasks) Text('- ' + t),
                    const SizedBox(height: 8),
                    Text('Chat', style: Theme.of(context).textTheme.titleSmall),
                    for (final c in snap.chat) Text(c),
                    if (_logOpen.value) ...[
                      const SizedBox(height: 8),
                      Text('Log', style: Theme.of(context).textTheme.titleSmall),
                      for (final l in snap.log) Text(l, style: const TextStyle(fontSize: 11)),
                    ],
                  ],
                );
              },
            ),
          ],
        ],
      ),
    );
  }

  Widget _keyField(TextEditingController c, String label, String hint) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: TextField(
        controller: c,
        obscureText: true,
        decoration: InputDecoration(labelText: label, hintText: hint),
      ),
    );
  }

  Widget _botRow(_Row row) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('@' + row.handle + '  [' + row.roles.join('/') + ']'),
            DropdownButton<_BackendChoice>(
              value: row.choice,
              isExpanded: true,
              items: _choices
                  .map((c) => DropdownMenuItem(
                        value: c,
                        child: Text(c.model),
                      ))
                  .toList(),
              onChanged: (v) => setState(() => row.choice = v ?? row.choice),
            ),
          ],
        ),
      ),
    );
  }
}
