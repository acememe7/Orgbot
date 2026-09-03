// ignore_for_file: prefer_interpolation_to_compose_strings

// OrgBot home: your bots, and rooms as group chats of those bots.
library;

import 'package:flutter/material.dart';

import '../engine/agent_cli.dart';
import '../orgbot/library.dart';
import '../orgbot/models.dart';
import '../orgbot/model_catalog.dart';
import 'brand.dart';
import 'engine_room_screen.dart';
import 'role_icons.dart';
import '../orgbot/roles.dart';

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
    await showDialog<void>(
      context: context,
      builder: (context) => _BotEditor(
        existing: existing,
        onSave: (bot) {
          if (existing == null) {
            widget.library.addBot(bot);
          } else {
            widget.library.updateBot(bot);
          }
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
                  // Each bot brings its own role, set where the bot is
                  // defined — the same reviewer reviews in every room.
                  for (final b in widget.library.bots)
                    CheckboxListTile(
                      dense: true,
                      value: selected.containsKey(b.id),
                      secondary: Icon(roleIcon(b.roleDef.icon), size: 20),
                      title: Text('@' + (b.handle.isEmpty ? b.name : b.handle)),
                      subtitle: Text(
                          b.roleDef.label + ' · ' + b.provider.label),
                      onChanged: (v) => setLocal(() {
                        if (v == true) {
                          selected[b.id] = b.role;
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
                    bots: selected.keys.map(RoomSlot.new).toList(),
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
      builder: (_) => EngineRoomScreen(room: room, library: widget.library),
    ));
  }
}

/// The bot editor.
///
/// Split out of the screen because it has real state of its own: the model
/// list is fetched from the provider (a CLI call or an HTTP round trip), so it
/// arrives after the dialog is already on screen.
class _BotEditor extends StatefulWidget {
  final BotDef? existing;
  final void Function(BotDef) onSave;
  const _BotEditor({required this.existing, required this.onSave});

  @override
  State<_BotEditor> createState() => _BotEditorState();
}

class _BotEditorState extends State<_BotEditor> {
  late final TextEditingController _name;
  late final TextEditingController _handle;
  late final TextEditingController _persona;
  late final TextEditingController _key;
  final TextEditingController _customModel = TextEditingController();

  late ProviderKind _provider;
  late String _role;
  RolePowers? _powers;
  String _model = '';
  bool _custom = false;

  ModelList? _models;
  bool _loadingModels = false;

  /// Sentinel for the "type your own id" row.
  static const String _customValue = '__custom__';

  @override
  void initState() {
    super.initState();
    final b = widget.existing;
    _name = TextEditingController(text: b?.name ?? '');
    _handle = TextEditingController(text: b?.handle ?? '');
    _persona = TextEditingController(text: b?.persona ?? '');
    _key = TextEditingController(text: b?.apiKey ?? '');
    _provider = b?.provider ?? ProviderKind.anthropic;
    _role = b?.role ?? Roles.defaultKey;
    _powers = b?.powers;
    _model = b?.model ?? '';
    _loadModels();
  }

  @override
  void dispose() {
    _name.dispose();
    _handle.dispose();
    _persona.dispose();
    _key.dispose();
    _customModel.dispose();
    super.dispose();
  }

  Future<void> _loadModels() async {
    setState(() {
      _loadingModels = true;
      _models = null;
    });
    final list = await ModelCatalog.load(_provider, apiKey: _key.text.trim());
    if (!mounted) return;
    setState(() {
      _models = list;
      _loadingModels = false;
      // An id that the provider does not offer is a custom one; keep it and
      // open on Custom rather than silently switching the bot's model.
      if (_model.isNotEmpty && !list.options.any((o) => o.id == _model)) {
        _custom = true;
        _customModel.text = _model;
      }
    });
  }

  void _changeProvider(ProviderKind next) {
    setState(() {
      _provider = next;
      // Model ids do not carry across providers.
      _model = ModelCatalog.defaultFor(next);
      _custom = false;
      _customModel.clear();
    });
    _loadModels();
  }

  String get _resolvedModel => _custom ? _customModel.text.trim() : _model;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return AlertDialog(
      title: Row(
        children: [
          const BrandMark(size: 24),
          const SizedBox(width: 10),
          Text(widget.existing == null ? 'New bot' : 'Edit bot'),
        ],
      ),
      content: SizedBox(
        width: 430,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: _name,
                textCapitalization: TextCapitalization.words,
                decoration: const InputDecoration(
                    labelText: 'Name', hintText: 'Iris'),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _handle,
                decoration: const InputDecoration(
                  labelText: 'Handle',
                  prefixText: '@',
                  hintText: 'iris',
                  helperText: 'How the other bots address it',
                ),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _persona,
                maxLines: 3,
                decoration: const InputDecoration(
                  labelText: 'Persona',
                  hintText: 'You write concise, accurate technical prose.',
                  alignLabelWithHint: true,
                ),
              ),
              const SizedBox(height: 18),
              Text('Job', style: Theme.of(context).textTheme.labelLarge),
              const SizedBox(height: 8),
              _rolePicker(scheme),
              const SizedBox(height: 18),
              Text('Brain', style: Theme.of(context).textTheme.labelLarge),
              const SizedBox(height: 8),
              DropdownButtonFormField<ProviderKind>(
                initialValue: _provider,
                decoration: const InputDecoration(
                    labelText: 'Provider', border: OutlineInputBorder()),
                items: ProviderKind.values
                    .map((k) => DropdownMenuItem(value: k, child: Text(k.label)))
                    .toList(),
                onChanged: (v) => _changeProvider(v ?? _provider),
              ),
              const SizedBox(height: 10),
              _authRow(scheme),
              if (_provider != ProviderKind.scripted) ...[
                const SizedBox(height: 10),
                _modelField(scheme),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel')),
        FilledButton(onPressed: _save, child: const Text('Save')),
      ],
    );
  }

  /// The job the bot holds, grouped the way an organisation is.
  Widget _rolePicker(ColorScheme scheme) {
    final role = Roles.byKey(_role);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        DropdownButtonFormField<String>(
          initialValue: _role,
          isExpanded: true,
          decoration: const InputDecoration(
              labelText: 'Role', border: OutlineInputBorder()),
          items: [
            for (final group in RoleGroup.values) ...[
              DropdownMenuItem<String>(
                enabled: false,
                value: '__group_${group.name}',
                child: Text(group.label.toUpperCase(),
                    style: TextStyle(
                        fontSize: 10,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.6,
                        color: scheme.onSurfaceVariant)),
              ),
              for (final r in Roles.inGroup(group))
                DropdownMenuItem<String>(
                  value: r.key,
                  child: Row(
                    children: [
                      Icon(roleIcon(r.icon), size: 16),
                      const SizedBox(width: 8),
                      Flexible(
                          child: Text(r.label, overflow: TextOverflow.ellipsis)),
                    ],
                  ),
                ),
            ],
          ],
          onChanged: (v) {
            if (v == null || v.startsWith('__group_')) return;
            // Changing the job resets any hand-tuned permissions, so the new
            // role's defaults are what actually apply.
            setState(() {
              _role = v;
              _powers = null;
            });
          },
        ),
        const SizedBox(height: 6),
        Text(role.summary,
            style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant)),
        const SizedBox(height: 8),
        _powerChips(scheme, role),
      ],
    );
  }

  /// What the role is allowed to do, and a way to override it.
  Widget _powerChips(ColorScheme scheme, RoleDef role) {
    final powers = _powers ?? role.powers;
    Widget chip(String label, bool on, RolePowers Function(bool) apply) =>
        FilterChip(
          label: Text(label, style: const TextStyle(fontSize: 11)),
          selected: on,
          visualDensity: VisualDensity.compact,
          onSelected: (v) => setState(() => _powers = apply(v)),
        );
    return Wrap(
      spacing: 6,
      runSpacing: 4,
      children: [
        chip('Creates tasks', powers.mayCreateTasks,
            (v) => powers.copyWith(mayCreateTasks: v)),
        chip('Inspects work', powers.mayInspect,
            (v) => powers.copyWith(mayInspect: v)),
        chip('Delivers', powers.maySubmit, (v) => powers.copyWith(maySubmit: v)),
      ],
    );
  }

  /// Sign-in status, or a key field — never both.
  Widget _authRow(ColorScheme scheme) {
    if (_provider.needsApiKey) {
      return TextField(
        controller: _key,
        obscureText: true,
        decoration: const InputDecoration(
          labelText: 'API key',
          border: OutlineInputBorder(),
          helperText: 'Stored on this device only',
        ),
        // A key unlocks the live model list, so reload once it is entered.
        onSubmitted: (_) => _loadModels(),
        onEditingComplete: _loadModels,
      );
    }
    if (_provider == ProviderKind.scripted) {
      return Text(
        'The demo bot follows a fixed script, so you can try a room before '
        'setting anything up.',
        style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
      );
    }

    final cli = _provider.cli!;
    final status = findCli(cli);
    if (status.available) {
      return StatusPill(
        color: Colors.green,
        label: 'Signed in via ${cli.binary} — no API key needed',
        icon: Icons.verified_user_outlined,
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const StatusPill(
          color: Colors.orange,
          label: 'CLI not found',
          icon: Icons.error_outline,
        ),
        const SizedBox(height: 6),
        Text(
          '${status.problem}\nInstall it and sign in with `${cli.binary}`, then '
          'reopen this dialog.',
          style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
        ),
      ],
    );
  }

  Widget _modelField(ColorScheme scheme) {
    if (_loadingModels) {
      return Row(
        children: [
          const SizedBox(
              width: 14, height: 14, child: CircularProgressIndicator(strokeWidth: 2)),
          const SizedBox(width: 10),
          Text('Loading models…',
              style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant)),
        ],
      );
    }
    final list = _models;
    if (list == null) return const SizedBox.shrink();

    if (list.options.isEmpty && !_custom) {
      // Nothing can be listed yet — say why rather than inventing options.
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(list.problem ?? 'No models available yet.',
              style: TextStyle(fontSize: 12, color: Colors.orange)),
          const SizedBox(height: 6),
          TextButton.icon(
            onPressed: () => setState(() => _custom = true),
            icon: const Icon(Icons.edit, size: 15),
            label: const Text('Enter a model id instead'),
          ),
        ],
      );
    }
    final value = _custom
        ? _customValue
        : (list.options.any((o) => o.id == _model)
            ? _model
            : (list.options.isEmpty ? _customValue : list.options.first.id));

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        DropdownButtonFormField<String>(
          initialValue: value,
          isExpanded: true,
          decoration: InputDecoration(
            labelText: 'Model',
            border: const OutlineInputBorder(),
            suffixIcon: IconButton(
              tooltip: 'Reload from the provider',
              icon: const Icon(Icons.refresh, size: 18),
              onPressed: () {
                ModelCatalog.invalidate();
                _loadModels();
              },
            ),
          ),
          items: [
            for (final option in list.options)
              DropdownMenuItem(
                value: option.id,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(option.label, overflow: TextOverflow.ellipsis),
                    if (option.note.isNotEmpty)
                      Text(option.note,
                          style: TextStyle(
                              fontSize: 11, color: scheme.onSurfaceVariant)),
                  ],
                ),
              ),
            const DropdownMenuItem(value: _customValue, child: Text('Custom…')),
          ],
          onChanged: (v) => setState(() {
            if (v == _customValue) {
              _custom = true;
            } else {
              _custom = false;
              _model = v ?? _model;
            }
          }),
        ),
        if (_custom) ...[
          const SizedBox(height: 10),
          TextField(
            controller: _customModel,
            decoration: const InputDecoration(
              labelText: 'Model id',
              hintText: 'exact id from your provider',
              border: OutlineInputBorder(),
            ),
          ),
        ],
        const SizedBox(height: 6),
        Text(
          list.source == ModelSource.live
              ? 'Loaded from ${_provider.label}.'
              : 'Showing built-in defaults — ${list.problem ?? "could not reach the provider"}.',
          style: TextStyle(
            fontSize: 11,
            color: list.source == ModelSource.live
                ? scheme.onSurfaceVariant
                : Colors.orange,
          ),
        ),
      ],
    );
  }

  void _save() {
    if (_name.text.trim().isEmpty) {
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('Give the bot a name')));
      return;
    }
    if (_custom && _resolvedModel.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Enter a model id, or pick one above')));
      return;
    }
    widget.onSave(BotDef(
      id: widget.existing?.id,
      name: _name.text.trim(),
      handle: _handle.text.trim(),
      persona: _persona.text.trim(),
      provider: _provider,
      model: _resolvedModel,
      apiKey: _key.text.trim(),
      role: _role,
      powers: _powers,
    ));
    Navigator.pop(context);
  }
}
