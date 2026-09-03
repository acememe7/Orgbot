// ignore_for_file: prefer_interpolation_to_compose_strings

// OrgBot home: your bots, and rooms as group chats of those bots.
library;

import 'package:flutter/material.dart';

import '../engine/engine_core.dart';
import '../engine/llm_provider.dart';
import '../engine/runner.dart';
import '../orgbot/demo_bot.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
import '../state/local_room_controller.dart';

class OrgBotScreen extends StatefulWidget {
  final Library library;
  const OrgBotScreen({super.key, required this.library});

  @override
  State<OrgBotScreen> createState() => _OrgBotScreenState();
}

class _OrgBotScreenState extends State<OrgBotScreen> {
  int _tab = 0;

  @override
  void initState() {
    super.initState();
    widget.library.load();
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.library,
      builder: (context, _) {
        final lib = widget.library;
        return Scaffold(
          appBar: AppBar(
            title: Text(_tab == 0 ? 'OrgBot · your bots' : 'OrgBot · rooms'),
            actions: [
              IconButton(
                icon: const Icon(Icons.add),
                tooltip: _tab == 0 ? 'New bot' : 'New room',
                onPressed: _tab == 0 ? _newBot : _newRoom,
              ),
            ],
          ),
          bottomNavigationBar: NavigationBar(
            selectedIndex: _tab,
            onDestinationSelected: (i) => setState(() => _tab = i),
            destinations: const [
              NavigationDestination(
                  icon: Icon(Icons.smart_toy_outlined),
                  selectedIcon: Icon(Icons.smart_toy),
                  label: 'Bots'),
              NavigationDestination(
                  icon: Icon(Icons.forum_outlined),
                  selectedIcon: Icon(Icons.forum),
                  label: 'Rooms'),
            ],
          ),
          body: _tab == 0 ? _botsList(lib) : _roomsList(lib),
        );
      },
    );
  }

  // ---- bots ----
  Widget _botsList(Library lib) {
    if (lib.bots.isEmpty) {
      return const Center(child: Text('No bots yet. Tap + to create one.'));
    }
    return ListView.builder(
      itemCount: lib.bots.length,
      itemBuilder: (context, i) {
        final b = lib.bots[i];
        return ListTile(
          leading: CircleAvatar(child: Text(b.name.isNotEmpty ? b.name[0] : '?')),
          title: Text('@' + (b.handle.isEmpty ? b.name : b.handle)),
          subtitle: Text(b.provider.label +
              (b.model.isNotEmpty ? ' · ' + b.model : '') +
              '\n' +
              b.persona),
          isThreeLine: b.persona.isNotEmpty,
          trailing: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              IconButton(
                  icon: const Icon(Icons.edit_outlined),
                  onPressed: () => _botDialog(b)),
              IconButton(
                  icon: const Icon(Icons.delete_outline),
                  onPressed: () => lib.removeBot(b.id)),
            ],
          ),
        );
      },
    );
  }

  void _newBot() => _botDialog(null);

  Future<void> _botDialog(BotDef? existing) async {
    final name = TextEditingController(text: existing?.name ?? '');
    final handle = TextEditingController(text: existing?.handle ?? '');
    final persona = TextEditingController(text: existing?.persona ?? '');
    final model = TextEditingController(text: existing?.model ?? '');
    final key = TextEditingController(text: existing?.apiKey ?? '');
    var provider = existing?.provider ?? ProviderKind.scripted;

    await showDialog<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setLocal) => AlertDialog(
          title: Text(existing == null ? 'New bot' : 'Edit bot'),
          content: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(controller: name, decoration: const InputDecoration(labelText: 'Name')),
                TextField(controller: handle, decoration: const InputDecoration(labelText: 'Handle (@mention)')),
                TextField(
                    controller: persona,
                    maxLines: 4,
                    decoration: const InputDecoration(
                        labelText: 'System prompt / persona')),
                DropdownButtonFormField<ProviderKind>(
                  initialValue: provider,
                  decoration: const InputDecoration(labelText: 'Provider'),
                  items: ProviderKind.values
                      .map((k) =>
                          DropdownMenuItem(value: k, child: Text(k.label)))
                      .toList(),
                  onChanged: (v) => setLocal(() => provider = v ?? provider),
                ),
                TextField(controller: model, decoration: const InputDecoration(labelText: 'Model (empty = default)')),
                if (provider != ProviderKind.scripted)
                  TextField(
                      controller: key,
                      obscureText: true,
                      decoration: const InputDecoration(labelText: 'API key')),
              ],
            ),
          ),
          actions: [
            TextButton(
                onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
            FilledButton(
                onPressed: () {
                  if (name.text.trim().isEmpty) return;
                  final bot = BotDef(
                    id: existing?.id,
                    name: name.text.trim(),
                    handle: handle.text.trim(),
                    persona: persona.text.trim(),
                    provider: provider,
                    model: model.text.trim(),
                    apiKey: key.text.trim(),
                  );
                  if (existing == null) {
                    widget.library.addBot(bot);
                  } else {
                    widget.library.updateBot(bot);
                  }
                  Navigator.pop(context);
                },
                child: const Text('Save')),
          ],
        ),
      ),
    );
  }

  // ---- rooms ----
  Widget _roomsList(Library lib) {
    if (lib.rooms.isEmpty) {
      return const Center(
          child: Text('No rooms yet. Tap + to open a group chat of your bots.'));
    }
    return ListView.builder(
      itemCount: lib.rooms.length,
      itemBuilder: (context, i) {
        final r = lib.rooms[i];
        return Card(
          margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: ListTile(
            title: Text(r.name),
            subtitle: Text('Goal: ' +
                r.goal +
                '\n' +
                r.bots.length.toString() +
                ' bots' +
                (r.tasks.isNotEmpty ? ' · ' + r.tasks.length.toString() + ' tasks' : '')),
            isThreeLine: true,
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                FilledButton(
                    onPressed: () => _runRoom(r), child: const Text('Start')),
                IconButton(
                    icon: const Icon(Icons.delete_outline),
                    onPressed: () => lib.removeRoom(r.id)),
              ],
            ),
          ),
        );
      },
    );
  }

  void _newRoom() => _roomDialog();

  Future<void> _roomDialog() async {
    final name = TextEditingController();
    final goal = TextEditingController(text: 'Write and ship the release notes.');
    final path = TextEditingController();
    final selected = <String, String>{}; // botId -> role

    await showDialog<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setLocal) => AlertDialog(
          title: const Text('New room — a group chat'),
          content: SizedBox(
            width: 460,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  TextField(controller: name, decoration: const InputDecoration(labelText: 'Room name')),
                  TextField(controller: goal, maxLines: 3, decoration: const InputDecoration(labelText: 'Goal / task')),
                  TextField(controller: path, decoration: const InputDecoration(labelText: 'Project path (desktop)')),
                  const Divider(),
                  Text('Who is in the room',
                      style: Theme.of(context).textTheme.titleSmall),
                  for (final b in widget.library.bots)
                    CheckboxListTile(
                      dense: true,
                      value: selected.containsKey(b.id),
                      title: Text('@' + (b.handle.isEmpty ? b.name : b.handle)),
                      subtitle: DropdownButtonFormField<String>(
                        initialValue: selected[b.id] ?? 'writer',
                        items: const [
                          DropdownMenuItem(value: 'writer', child: Text('Writer')),
                          DropdownMenuItem(value: 'coordinator', child: Text('Coordinator')),
                          DropdownMenuItem(value: 'reviewer', child: Text('Inspector')),
                        ],
                        onChanged: (v) => setLocal(() => selected[b.id] = v ?? 'writer'),
                      ),
                      onChanged: (v) => setLocal(() {
                        if (v == true) {
                          selected[b.id] = 'writer';
                        } else {
                          selected.remove(b.id);
                        }
                      }),
                    ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
                onPressed: () => Navigator.pop(context), child: const Text('Cancel')),
            FilledButton(
                onPressed: () {
                  if (name.text.trim().isEmpty) {
                    ScaffoldMessenger.of(context).showSnackBar(
                        const SnackBar(content: Text('Give the room a name')));
                    return;
                  }
                  final room = RoomDef(
                    name: name.text.trim(),
                    goal: goal.text.trim(),
                    path: path.text.trim(),
                    tasks: [goal.text.trim()].where((t) => t.isNotEmpty).toList(),
                    bots: selected.entries
                        .map((e) => RoomSlot(e.key, e.value))
                        .toList(),
                  );
                  widget.library.addRoom(room);
                  Navigator.pop(context);
                },
                child: const Text('Create')),
          ],
        ),
      ),
    );
  }

  void _runRoom(RoomDef room) {
    Navigator.of(context).push(MaterialPageRoute(
      builder: (_) => RunningRoomPage(room: room, library: widget.library),
    ));
  }
}

/// Runs a saved room on-device: bots (or scripted stand-ins) work the board.
class RunningRoomPage extends StatefulWidget {
  final RoomDef room;
  final Library library;
  const RunningRoomPage({super.key, required this.room, required this.library});

  @override
  State<RunningRoomPage> createState() => _RunningRoomPageState();
}

class _RunningRoomPageState extends State<RunningRoomPage> {
  LocalRoomController? _controller;

  @override
  void initState() {
    super.initState();
    final engine = Engine(name: widget.room.name, goal: widget.room.goal);
    for (final t in widget.room.tasks) {
      engine.seedTask(t, t, requiredRole: 'writer');
    }
    final runner = Runner(engine);
    for (final slot in widget.room.bots) {
      final bot = widget.library.botById(slot.botId);
      if (bot == null) continue;
      final roles = [slot.role];
      final provider = bot.provider == ProviderKind.scripted
          ? ScriptedRoomBot(bot.handle.isEmpty ? bot.name : bot.handle, slot.role)
          : _realBot(bot, slot.role);
      runner.seat(
        bot.handle.isEmpty ? bot.name : bot.handle,
        roles,
        provider,
        mayCreate: slot.role == 'coordinator',
        maySubmit: slot.role != 'reviewer',
        mayInspect: slot.role == 'reviewer',
      );
    }
    final controller = LocalRoomController(engine: engine, runner: runner);
    _controller = controller;
    controller.start();
  }

  BotProvider _realBot(BotDef bot, String role) {
    final system = 'You are ' +
        bot.name +
        ' (' +
        role +
        ') in a room of bots sharing a task board.\n' +
        bot.persona;
    switch (bot.provider) {
      case ProviderKind.anthropic:
        return LlmBot(LlmConfig(
            kind: LlmKind.anthropic,
            apiKey: bot.apiKey,
            model: bot.model.isEmpty ? 'claude-sonnet-5' : bot.model,
            system: system));
      case ProviderKind.deepseek:
        return LlmBot(LlmConfig(
            kind: LlmKind.openAiCompatible,
            apiKey: bot.apiKey,
            model: bot.model.isEmpty ? 'deepseek-chat' : bot.model,
            baseUrl: 'https://api.deepseek.com/v1',
            system: system));
      case ProviderKind.openai:
        return LlmBot(LlmConfig(
            kind: LlmKind.openAiCompatible,
            apiKey: bot.apiKey,
            model: bot.model.isEmpty ? 'gpt-5.2-codex' : bot.model,
            baseUrl: 'https://api.openai.com/v1',
            system: system));
      case ProviderKind.scripted:
        return ScriptedRoomBot(bot.handle.isEmpty ? bot.name : bot.handle, role);
    }
  }

  @override
  void dispose() {
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    if (controller == null) {
      return const Scaffold(body: Center(child: Text('no bots in this room')));
    }
    return Scaffold(
      appBar: AppBar(title: Text('Room: ' + widget.room.name)),
      body: AnimatedBuilder(
        animation: controller,
        builder: (context, _) {
          final snap = controller.snapshot();
          return Column(
            children: [
              Padding(
                padding: const EdgeInsets.all(8),
                child: Wrap(spacing: 8, children: [
                  Chip(label: Text('phase: ' + snap.phase)),
                  if (controller.outcome != null)
                    Chip(label: Text('outcome: ' + controller.outcome!)),
                  if (controller.error != null)
                    Chip(label: Text('error: ' + controller.error!)),
                  if (controller.state == LocalRunState.running)
                    TextButton(
                        onPressed: controller.pause, child: const Text('Pause'))
                  else if (controller.state == LocalRunState.paused)
                    TextButton(
                        onPressed: controller.resume, child: const Text('Resume')),
                ]),
              ),
              Expanded(
                child: ListView(
                  padding: const EdgeInsets.all(12),
                  children: [
                    Text('Board', style: Theme.of(context).textTheme.titleSmall),
                    for (final t in snap.tasks) Text('- ' + t),
                    const SizedBox(height: 12),
                    Text('Chat', style: Theme.of(context).textTheme.titleSmall),
                    for (final c in snap.chat) Text(c),
                  ],
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}
