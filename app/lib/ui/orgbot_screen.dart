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
import '../orgbot/model_catalog.dart';
import '../state/local_room_controller.dart';
import 'brand.dart';

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
            titleSpacing: 12,
            title: Row(
              children: [
                const BrandMark(size: 28),
                const SizedBox(width: 10),
                Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Text('OrgBot', style: TextStyle(fontSize: 17)),
                    Text(
                      _tab == 0 ? 'Your bots' : 'Your rooms',
                      style: TextStyle(
                        fontSize: 11,
                        color: Theme.of(context).colorScheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ),
              ],
            ),
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
      return EmptyState(
        icon: Icons.smart_toy_outlined,
        title: 'No bots yet',
        message: 'A bot is a model with a name, a persona, and a job. '
            'Create a few, then put them in a room together.',
        actionLabel: 'Create a bot',
        onAction: _newBot,
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.fromLTRB(12, 12, 12, 88),
      itemCount: lib.bots.length,
      itemBuilder: (context, i) {
        final b = lib.bots[i];
        final handle = b.handle.isEmpty ? b.name : b.handle;
        final scheme = Theme.of(context).colorScheme;
        final needsKey = b.needsKey && b.apiKey.isEmpty;
        return Card(
          margin: const EdgeInsets.only(bottom: 10),
          elevation: 0,
          color: scheme.surfaceContainerHighest,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(14),
            side: BorderSide(color: scheme.outlineVariant),
          ),
          child: InkWell(
            borderRadius: BorderRadius.circular(14),
            onTap: () => _botDialog(b),
            child: Padding(
              padding: const EdgeInsets.all(14),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      BotAvatar(handle: handle),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text('@' + handle,
                                style: const TextStyle(
                                    fontWeight: FontWeight.w700, fontSize: 15)),
                            const SizedBox(height: 2),
                            Text(
                              b.provider.label +
                                  ' · ' +
                                  ModelCatalog.labelFor(b.provider, b.model),
                              style: TextStyle(
                                  fontSize: 12, color: scheme.onSurfaceVariant),
                            ),
                          ],
                        ),
                      ),
                      IconButton(
                        tooltip: 'Delete',
                        icon: const Icon(Icons.delete_outline),
                        onPressed: () => lib.removeBot(b.id),
                      ),
                    ],
                  ),
                  if (b.persona.isNotEmpty) ...[
                    const SizedBox(height: 10),
                    Text(
                      b.persona,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: scheme.onSurfaceVariant),
                    ),
                  ],
                  if (needsKey) ...[
                    const SizedBox(height: 10),
                    const StatusPill(
                      color: Colors.orange,
                      label: 'Needs an API key',
                      icon: Icons.key_off_outlined,
                    ),
                  ],
                ],
              ),
            ),
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
    final customModel = TextEditingController();
    final key = TextEditingController(text: existing?.apiKey ?? '');
    var provider = existing?.provider ?? ProviderKind.anthropic;

    // The picker holds a model id, or the sentinel below for "type your own".
    // An existing bot pointed at something not in the catalog opens on Custom
    // with its id filled in, so editing never silently changes the model.
    const custom = '__custom__';
    var selected = existing == null || existing.model.isEmpty
        ? ModelCatalog.defaultFor(provider)
        : (ModelCatalog.isKnown(existing.provider, existing.model)
            ? existing.model
            : custom);
    if (selected == custom) customModel.text = existing!.model;

    String resolvedModel() =>
        selected == custom ? customModel.text.trim() : selected;

    await showDialog<void>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setLocal) {
          final options = ModelCatalog.forProvider(provider);
          final scheme = Theme.of(context).colorScheme;
          return AlertDialog(
            title: Row(
              children: [
                const BrandMark(size: 24),
                const SizedBox(width: 10),
                Text(existing == null ? 'New bot' : 'Edit bot'),
              ],
            ),
            content: SizedBox(
              width: 420,
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    TextField(
                      controller: name,
                      textCapitalization: TextCapitalization.words,
                      decoration: const InputDecoration(
                        labelText: 'Name',
                        hintText: 'Iris',
                      ),
                    ),
                    const SizedBox(height: 10),
                    TextField(
                      controller: handle,
                      decoration: const InputDecoration(
                        labelText: 'Handle',
                        hintText: 'iris',
                        prefixText: '@',
                        helperText: 'How the other bots address it',
                      ),
                    ),
                    const SizedBox(height: 10),
                    TextField(
                      controller: persona,
                      maxLines: 3,
                      decoration: const InputDecoration(
                        labelText: 'Persona',
                        hintText: 'You write concise, accurate technical prose.',
                        helperText: 'Standing instructions, given every turn',
                        alignLabelWithHint: true,
                      ),
                    ),
                    const SizedBox(height: 18),
                    Text('Brain',
                        style: Theme.of(context).textTheme.labelLarge),
                    const SizedBox(height: 8),
                    DropdownButtonFormField<ProviderKind>(
                      initialValue: provider,
                      decoration: const InputDecoration(
                        labelText: 'Provider',
                        border: OutlineInputBorder(),
                      ),
                      items: ProviderKind.values
                          .map((k) =>
                              DropdownMenuItem(value: k, child: Text(k.label)))
                          .toList(),
                      onChanged: (v) => setLocal(() {
                        provider = v ?? provider;
                        // Models do not carry across providers, so reset to
                        // the new provider's default rather than leaving an
                        // id that its API will reject.
                        selected = ModelCatalog.defaultFor(provider);
                        customModel.clear();
                      }),
                    ),
                    if (options.isNotEmpty) ...[
                      const SizedBox(height: 10),
                      DropdownButtonFormField<String>(
                        initialValue: selected,
                        isExpanded: true,
                        decoration: const InputDecoration(
                          labelText: 'Model',
                          border: OutlineInputBorder(),
                        ),
                        items: [
                          for (final option in options)
                            DropdownMenuItem(
                              value: option.id,
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  Text(option.label),
                                  Text(
                                    option.note,
                                    style: TextStyle(
                                        fontSize: 11,
                                        color: scheme.onSurfaceVariant),
                                  ),
                                ],
                              ),
                            ),
                          const DropdownMenuItem(
                            value: custom,
                            child: Text('Custom…'),
                          ),
                        ],
                        onChanged: (v) =>
                            setLocal(() => selected = v ?? selected),
                      ),
                    ],
                    if (selected == custom) ...[
                      const SizedBox(height: 10),
                      TextField(
                        controller: customModel,
                        decoration: const InputDecoration(
                          labelText: 'Model id',
                          hintText: 'exact id from your provider',
                          border: OutlineInputBorder(),
                        ),
                      ),
                    ],
                    if (provider != ProviderKind.scripted) ...[
                      const SizedBox(height: 10),
                      TextField(
                        controller: key,
                        obscureText: true,
                        decoration: const InputDecoration(
                          labelText: 'API key',
                          border: OutlineInputBorder(),
                          helperText: 'Stored on this device only',
                        ),
                      ),
                    ] else ...[
                      const SizedBox(height: 10),
                      Text(
                        'The demo bot needs no key or model — it follows a '
                        'fixed script, so you can try a room before adding one.',
                        style: TextStyle(
                            fontSize: 12, color: scheme.onSurfaceVariant),
                      ),
                    ],
                  ],
                ),
              ),
            ),
            actions: [
              TextButton(
                  onPressed: () => Navigator.pop(context),
                  child: const Text('Cancel')),
              FilledButton(
                  onPressed: () {
                    if (name.text.trim().isEmpty) {
                      ScaffoldMessenger.of(context).showSnackBar(
                          const SnackBar(content: Text('Give the bot a name')));
                      return;
                    }
                    if (selected == custom && resolvedModel().isEmpty) {
                      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(
                          content: Text('Enter a model id, or pick one above')));
                      return;
                    }
                    final bot = BotDef(
                      id: existing?.id,
                      name: name.text.trim(),
                      handle: handle.text.trim(),
                      persona: persona.text.trim(),
                      provider: provider,
                      model: resolvedModel(),
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
          );
        },
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
            model: bot.model.isEmpty
                ? ModelCatalog.defaultFor(ProviderKind.anthropic)
                : bot.model,
            system: system));
      case ProviderKind.deepseek:
        return LlmBot(LlmConfig(
            kind: LlmKind.openAiCompatible,
            apiKey: bot.apiKey,
            model: bot.model.isEmpty
                ? ModelCatalog.defaultFor(ProviderKind.deepseek)
                : bot.model,
            baseUrl: 'https://api.deepseek.com/v1',
            system: system));
      case ProviderKind.openai:
        return LlmBot(LlmConfig(
            kind: LlmKind.openAiCompatible,
            apiKey: bot.apiKey,
            model: bot.model.isEmpty
                ? ModelCatalog.defaultFor(ProviderKind.openai)
                : bot.model,
            baseUrl: 'https://api.openai.com/v1',
            system: system));
      case ProviderKind.scripted:
        return ScriptedRoomBot(bot.handle.isEmpty ? bot.name : bot.handle, role);
    }
  }

  final TextEditingController _input = TextEditingController();
  final ScrollController _scroll = ScrollController();
  bool _showBoard = false;
  int _seenMessages = 0;

  @override
  void dispose() {
    _input.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _send() {
    final controller = _controller;
    if (controller == null) return;
    final text = _input.text.trim();
    if (text.isEmpty) return;
    _input.clear();
    controller.say(text);
  }

  /// Follow the conversation as it grows, but leave the scroll alone while a
  /// person is reading back through it.
  void _followTranscript(int count) {
    if (count == _seenMessages) return;
    _seenMessages = count;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.animateTo(
          _scroll.position.maxScrollExtent,
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOut,
        );
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    if (controller == null) {
      return Scaffold(
        appBar: AppBar(title: Text(widget.room.name)),
        body: const EmptyState(
          icon: Icons.group_off_outlined,
          title: 'No bots in this room',
          message: 'Edit the room and add at least one bot, then start it again.',
        ),
      );
    }
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final messages = controller.engine.messages;
        _followTranscript(messages.length);
        return Scaffold(
          appBar: AppBar(
            titleSpacing: 8,
            title: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(widget.room.name, style: const TextStyle(fontSize: 17)),
                Text(
                  widget.room.goal,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 11,
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                  ),
                ),
              ],
            ),
            actions: [
              _runPill(controller),
              IconButton(
                tooltip: _showBoard ? 'Hide the board' : 'Show the board',
                icon: Icon(_showBoard ? Icons.checklist : Icons.checklist_outlined),
                onPressed: () => setState(() => _showBoard = !_showBoard),
              ),
              if (controller.state == LocalRunState.running)
                IconButton(
                    tooltip: 'Pause',
                    icon: const Icon(Icons.pause),
                    onPressed: controller.pause)
              else if (controller.state == LocalRunState.paused)
                IconButton(
                    tooltip: 'Resume',
                    icon: const Icon(Icons.play_arrow),
                    onPressed: controller.resume),
              const SizedBox(width: 4),
            ],
          ),
          body: Column(
            children: [
              if (controller.error != null) _errorBanner(controller.error!),
              if (_showBoard) _board(controller),
              Expanded(child: _transcript(controller, messages)),
              const Divider(height: 1),
              _composer(controller),
            ],
          ),
        );
      },
    );
  }

  Widget _runPill(LocalRoomController controller) {
    final (color, label) = switch (controller.state) {
      LocalRunState.running => (Colors.blue, 'working'),
      LocalRunState.paused => (Colors.amber, 'paused'),
      LocalRunState.idle => (Colors.green, 'ready'),
      LocalRunState.finished => (Colors.green, 'done'),
      LocalRunState.failed => (Colors.red, 'failed'),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Center(child: StatusPill(color: color, label: label)),
    );
  }

  Widget _errorBanner(String error) {
    return Builder(builder: (context) {
      final scheme = Theme.of(context).colorScheme;
      return Container(
        width: double.infinity,
        color: scheme.errorContainer,
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        child: Row(
          children: [
            Icon(Icons.error_outline, size: 18, color: scheme.onErrorContainer),
            const SizedBox(width: 10),
            Expanded(
              child: Text(error,
                  style: TextStyle(fontSize: 12, color: scheme.onErrorContainer)),
            ),
          ],
        ),
      );
    });
  }

  Widget _board(LocalRoomController controller) {
    final scheme = Theme.of(context).colorScheme;
    final tasks = controller.engine.tasks.values.toList();
    return Container(
      constraints: const BoxConstraints(maxHeight: 220),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
      ),
      child: tasks.isEmpty
          ? Padding(
              padding: const EdgeInsets.all(16),
              child: Text('The board is empty.',
                  style: TextStyle(color: scheme.onSurfaceVariant)),
            )
          : ListView(
              padding: const EdgeInsets.all(12),
              children: [
                for (final t in tasks)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 8),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        StatusPill(
                            color: _taskColor(t.state.name),
                            label: t.state.name),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(t.title,
                                  style: const TextStyle(
                                      fontSize: 13, fontWeight: FontWeight.w600)),
                              if (t.assignee != null)
                                Text(
                                  '@' +
                                      (controller.engine.agents[t.assignee]?.handle ??
                                          '?'),
                                  style: TextStyle(
                                      fontSize: 11,
                                      color: scheme.onSurfaceVariant),
                                ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
    );
  }

  static Color _taskColor(String state) {
    switch (state) {
      case 'inProgress':
      case 'claimed':
        return Colors.blue;
      case 'blocked':
        return Colors.amber;
      case 'inReview':
        return Colors.purple;
      case 'approved':
        return Colors.teal;
      case 'submitted':
        return Colors.green;
      case 'failed':
      case 'cancelled':
        return Colors.red;
      default:
        return Colors.blueGrey;
    }
  }

  Widget _transcript(LocalRoomController controller, List<Message> messages) {
    if (messages.isEmpty) {
      return EmptyState(
        icon: Icons.forum_outlined,
        title: 'The room is quiet',
        message: controller.state == LocalRunState.running
            ? 'The bots are getting started. Anything you say goes to all of them.'
            : 'Say something to get the bots going.',
      );
    }
    return ListView.builder(
      controller: _scroll,
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      itemCount: messages.length,
      itemBuilder: (context, i) => _bubble(controller, messages[i]),
    );
  }

  Widget _bubble(LocalRoomController controller, Message message) {
    final scheme = Theme.of(context).colorScheme;
    final isBot = message.author.startsWith('@');
    final handle = isBot ? message.author.substring(1) : message.author;
    final color = isBot ? handleColor(handle) : scheme.primary;

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (isBot)
            BotAvatar(handle: handle, radius: 15)
          else
            CircleAvatar(
              radius: 15,
              backgroundColor: scheme.primary.withValues(alpha: 0.18),
              child: Icon(Icons.person, size: 16, color: scheme.primary),
            ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  message.author,
                  style: TextStyle(
                      fontWeight: FontWeight.w700, fontSize: 12.5, color: color),
                ),
                const SizedBox(height: 3),
                SelectableText(
                  message.body,
                  style: const TextStyle(fontSize: 14, height: 1.38),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _composer(LocalRoomController controller) {
    final busy = controller.state == LocalRunState.running;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Expanded(
            child: TextField(
              controller: _input,
              minLines: 1,
              maxLines: 5,
              textInputAction: TextInputAction.send,
              onSubmitted: (_) => _send(),
              decoration: InputDecoration(
                hintText: busy
                    ? 'The bots are working — you can still chime in'
                    : 'Message the room…',
                filled: true,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(24),
                  borderSide: BorderSide.none,
                ),
                contentPadding:
                    const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
              ),
            ),
          ),
          const SizedBox(width: 8),
          FilledButton(
            onPressed: _send,
            style: FilledButton.styleFrom(
              shape: const CircleBorder(),
              padding: const EdgeInsets.all(15),
            ),
            child: const Icon(Icons.arrow_upward, size: 19),
          ),
        ],
      ),
    );
  }
}
