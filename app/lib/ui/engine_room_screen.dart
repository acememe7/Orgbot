// A room, running on the real engine.
//
// Three regions, because a room has three questions: what is being said
// (chat), who is doing what right now (the activity rail), and what the work
// actually is (the board). The activity rail is the one that was missing —
// most of what a room does is work, not talk, and none of it was visible.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'widgets/objectives_pane.dart';
import 'dart:convert';
import 'dart:io';
import 'dart:async';
import 'package:flutter/material.dart';

import '../orgbot/library.dart';
import '../orgbot/models.dart' as defs;
import '../engine/agent_cli.dart';
import '../state/engine_room_controller.dart';
import '../state/room_archive.dart';
import '../workspace/workspace_screen.dart';
import '../workspace/workspace.dart';
import '../workspace/attachments.dart';
import '../protocol/models.dart' show ChatFile;
import '../orgbot/room_check.dart';
import '../state/room_store.dart';
import 'brand.dart';
import 'room_widgets.dart';
import 'polish.dart';

/// Runs a saved room on the engine and shows it.
class EngineRoomScreen extends StatefulWidget {
  const EngineRoomScreen({
    super.key,
    required this.room,
    required this.library,
    this.displayName,
  });

  final defs.RoomDef room;
  final Library library;
  final String? displayName;

  @override
  State<EngineRoomScreen> createState() => _EngineRoomScreenState();
}

class _EngineRoomScreenState extends State<EngineRoomScreen> {
  /// Whether the Work tab is showing every record rather than the
  /// objectives they roll up into.
  bool _showAllRecords = false;

  late final EngineRoomController _controller;
  final TextEditingController _input = TextEditingController();
  final ScrollController _scroll = ScrollController();
  late final TextEditingController _memory;
  bool _savingMemory = false;
  bool _sending = false;
  bool _preparingAttachments = false;
  late final RoomAttachments _attachments;
  List<ChatFile> _files = [];
  String? _attachmentRoot;
  bool _checkingFreshRoom = false;
  bool _needsStartConfirmation = false;
  String? _startCheckError;
  bool _startRequested = false;

  /// Below this the rail becomes a tab rather than a column.
  static const double _wide = 1000;

  @override
  void initState() {
    super.initState();
    _controller = widget.library.session(widget.room);
    _needsStartConfirmation = canRunAgentClis && _desktopStartable;
    _checkingFreshRoom = _needsStartConfirmation;
    _attachments = RoomAttachments(widget.room.id);
    if (Platform.isAndroid) {
      _preparingAttachments = true;
      _loadAttachments();
    }
    _input.text = _controller.draft;
    _memory = TextEditingController(text: widget.room.memory);
    _controller.trustHost = (host, type, key) async =>
        mounted && await trustWorkspaceHost(context, host, type, key);
    _controller.addListener(_onChange);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (!canRunAgentClis) {
        unawaited(_controller.start());
      } else if (_desktopStartable) {
        unawaited(_checkFreshRoomHistory());
      }
    });
  }

  bool get _desktopStartable => const [
    RoomLaunch.idle,
    RoomLaunch.closed,
    RoomLaunch.failed,
  ].contains(_controller.phase);

  Future<void> _checkFreshRoomHistory() async {
    setState(() => _checkingFreshRoom = true);
    try {
      final state = await RoomArchive.stateFile(widget.room.id);
      if (!mounted) return;
      final hasSavedHistory = await state.exists();
      if (!mounted) return;
      if (hasSavedHistory) {
        // The controller restores saved frames as a paused room; it does not
        // replay model or tool calls.
        setState(() => _needsStartConfirmation = false);
        unawaited(_controller.start());
      } else {
        setState(() => _needsStartConfirmation = true);
      }
    } catch (error) {
      if (mounted) {
        setState(() {
          _startCheckError = error.toString();
          _needsStartConfirmation = true;
        });
      }
    } finally {
      if (mounted) setState(() => _checkingFreshRoom = false);
    }
  }

  void _startFreshRoom() {
    if (_startRequested || !_needsStartConfirmation) return;
    setState(() {
      _startRequested = true;
      _needsStartConfirmation = false;
    });
    unawaited(_controller.start());
  }

  void _onChange() {
    if (mounted) setState(() {});
  }

  Future<void> _loadAttachments() async {
    try {
      final root = (await _attachments.directory).path;
      final files = await _attachments.loadDraft();
      if (mounted) {
        setState(() {
          _attachmentRoot = root;
          _files = files;
        });
      }
    } catch (e) {
      _attachmentError(e);
    } finally {
      if (mounted) setState(() => _preparingAttachments = false);
    }
  }

  void _attachmentError(Object error) {
    if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(error.toString())));
    }
  }

  Future<void> _pickAttachments() async {
    setState(() => _preparingAttachments = true);
    try {
      final files = [..._files, ...await _attachments.pick()];
      await _attachments.saveDraft(files);
      if (mounted) setState(() => _files = files);
    } catch (e) {
      _attachmentError(e);
    } finally {
      if (mounted) setState(() => _preparingAttachments = false);
    }
  }

  Future<void> _removeAttachment(ChatFile file) async {
    if (_preparingAttachments) return;
    setState(() => _preparingAttachments = true);
    final files = _files.where((f) => f.id != file.id).toList();
    try {
      await _attachments.saveDraft(files);
      if (mounted) setState(() => _files = files);
    } catch (e) {
      _attachmentError(e);
    } finally {
      if (mounted) setState(() => _preparingAttachments = false);
    }
  }

  Future<void> _openAttachment(ChatFile file) async {
    try {
      await _attachments.open(file);
    } catch (e) {
      _attachmentError(e);
    }
  }

  @override
  void dispose() {
    _controller.removeListener(_onChange);
    _controller.draft = _input.text;
    _controller.trustHost = null;
    _input.dispose();
    _memory.dispose();
    _scroll.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    if (_preparingAttachments || _sending) return;
    if (!_controller.isRunning) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Resume the room before sending. Your draft is kept.'),
        ),
      );
      return;
    }
    if (_files.length > RoomAttachments.maxFiles) {
      _attachmentError(
        'Send up to five files per message. Remove ${_files.length - RoomAttachments.maxFiles} to continue.',
      );
      return;
    }
    final text = _input.text.trim().isEmpty && _files.isNotEmpty
        ? 'Please review the attached files.'
        : _input.text.trim();
    if (text.isEmpty) return;
    final files = List<ChatFile>.of(_files);
    setState(() => _sending = true);
    try {
      if (!await _controller.say(
        widget.displayName ?? widget.library.userName,
        text,
        files: files,
      )) {
        _attachmentError(
          _controller.error ??
              'Message was not accepted. Your draft and attachments are kept.',
        );
        return;
      }
      _controller.draft = '';
      if (mounted) {
        _input.clear();
        setState(() => _files = []);
      }
      if (files.isNotEmpty) await _attachments.saveDraft([]);
    } catch (e) {
      _attachmentError(e);
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = _controller.store;
    final wide = MediaQuery.sizeOf(context).width >= _wide;

    return Scaffold(
      appBar: _appBar(store),
      body: store != null
          ? _room(store, wide)
          : _checkingFreshRoom
          ? _freshRoomStart()
          : _needsStartConfirmation
          ? _freshRoomStart()
          : switch (_controller.phase) {
              RoomLaunch.failed => _failure(),
              RoomLaunch.running when store != null => _room(store, wide),
              _ => _starting(),
            },
    );
  }

  Widget _freshRoomStart() {
    final scheme = Theme.of(context).colorScheme;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 460),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.play_circle_outline, size: 42, color: scheme.primary),
              const SizedBox(height: 14),
              Text(
                'Ready to start this room?',
                style: Theme.of(context).textTheme.titleLarge,
              ),
              const SizedBox(height: 10),
              const Text(
                'Provider subscription or API allowance is used while agents '
                'work. Viewing an idle room does not use it. The turn limit '
                'pauses the room by turn count; it is not an exact token cap.',
                textAlign: TextAlign.center,
              ),
              if (_checkingFreshRoom) ...[
                const SizedBox(height: 12),
                const Text('Checking saved room history…'),
              ],
              if (_startCheckError != null) ...[
                const SizedBox(height: 12),
                Text(
                  'Could not check saved history: $_startCheckError',
                  style: TextStyle(color: scheme.error),
                  textAlign: TextAlign.center,
                ),
              ],
              const SizedBox(height: 20),
              FilledButton.icon(
                key: const ValueKey('start-fresh-room'),
                onPressed: _checkingFreshRoom ? null : _startFreshRoom,
                icon: const Icon(Icons.play_arrow),
                label: const Text('Start room'),
              ),
            ],
          ),
        ),
      ),
    );
  }

  PreferredSizeWidget _appBar(RoomStore? store) {
    final scheme = Theme.of(context).colorScheme;
    final state = _controller.phase == RoomLaunch.failed
        ? 'Could not start'
        : !_controller.isRunning
        ? 'Saved'
        : store?.runStateLabel ?? 'Active';
    return AppBar(
      titleSpacing: 0,
      toolbarHeight: 68,
      title: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            widget.room.name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
          ),
          Text(
            '${widget.room.bots.length} ${widget.room.bots.length == 1 ? 'bot' : 'bots'} · $state',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
          ),
        ],
      ),
      actions: [
        if (store != null)
          IconButton(
            tooltip: !_controller.isRunning
                ? 'Retry'
                : store.isCompleted
                ? 'Continue'
                : store.isPaused
                ? 'Resume'
                : 'Pause',
            icon: Icon(
              !_controller.isRunning || store.canResume
                  ? Icons.play_arrow_rounded
                  : Icons.pause_rounded,
            ),
            onPressed: _controller.isRunning
                ? _controller.togglePause
                : _controller.start,
          ),
        PopupMenuButton<String>(
          tooltip: 'Room actions',
          onSelected: (value) async {
            if (value == 'reconnect') await _controller.reconnectWorkspace();
            if (value == 'stop') {
              await widget.library.stopRoom(widget.room.id);
              if (mounted) Navigator.pop(context);
            }
          },
          itemBuilder: (_) => [
            if (_controller.workspace != null)
              const PopupMenuItem(
                value: 'reconnect',
                child: Text('Reconnect computer'),
              ),
            const PopupMenuItem(value: 'stop', child: Text('Stop and save')),
          ],
        ),
      ],
    );
  }

  // ---- states ----

  Widget _starting() {
    final scheme = Theme.of(context).colorScheme;
    final steps = [
      ('Preparing the room', RoomLaunch.generating),
      ('Starting the engine', RoomLaunch.starting),
      ('Connecting', RoomLaunch.connecting),
    ];
    final current = _controller.phase.index;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 340),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Center(child: BrandMark(size: 56)),
            const SizedBox(height: 24),
            for (final step in steps)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 6),
                child: Row(
                  children: [
                    SizedBox(
                      width: 18,
                      height: 18,
                      child: current > step.$2.index
                          ? Icon(Icons.check, size: 16, color: scheme.primary)
                          : current == step.$2.index
                          ? const CircularProgressIndicator(strokeWidth: 2)
                          : Icon(
                              Icons.circle_outlined,
                              size: 14,
                              color: scheme.outlineVariant,
                            ),
                    ),
                    const SizedBox(width: 12),
                    Text(
                      step.$1,
                      style: TextStyle(
                        color: current >= step.$2.index
                            ? scheme.onSurface
                            : scheme.onSurfaceVariant,
                      ),
                    ),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _failure() {
    final scheme = Theme.of(context).colorScheme;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 460),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.error_outline, size: 44, color: scheme.error),
              const SizedBox(height: 14),
              Text(
                'The room could not start',
                style: Theme.of(context).textTheme.titleMedium,
              ),
              const SizedBox(height: 10),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: scheme.surfaceContainerHighest,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: SelectableText(
                  _controller.error ?? 'unknown error',
                  style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
                ),
              ),
              const SizedBox(height: 18),
              FilledButton.icon(
                onPressed: _controller.start,
                icon: const Icon(Icons.refresh),
                label: const Text('Try again'),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _room(RoomStore store, bool wide) {
    final problem = _controller.error ?? store.lastError;
    final chat = Column(
      children: [
        if (!_controller.isRunning && _controller.phase != RoomLaunch.failed)
          const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: Transcript(
            store: store,
            controller: _scroll,
            attachmentRoot: _attachmentRoot,
            onOpenAttachment: _openAttachment,
          ),
        ),
        Composer(
          controller: _input,
          onSend: _send,
          mentions: [for (final bot in store.agents) bot.handle],
          attachments: _files,
          attachmentRoot: _attachmentRoot,
          onAttach: Platform.isAndroid ? _pickAttachments : null,
          onRemoveAttachment: _removeAttachment,
          onOpenAttachment: _openAttachment,
          busy: _preparingAttachments || _sending,
          busyLabel: _sending ? 'Sending…' : 'Preparing attachments…',
        ),
      ],
    );
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 1100),
        child: DefaultTabController(
          length: 4,
          animationDuration: motionDuration(context, 320),
          child: Column(
            children: [
              if (problem != null)
                MaterialBanner(
                  content: Text(
                    '${!_controller.isRunning ? 'Showing saved history. ' : ''}$problem',
                  ),
                  actions: [
                    if (!_controller.isRunning)
                      TextButton(
                        onPressed: _controller.start,
                        child: const Text('Retry'),
                      ),
                    TextButton(
                      onPressed: () => setState(() {
                        _controller.error = null;
                        store.lastError = null;
                      }),
                      child: const Text('Dismiss'),
                    ),
                  ],
                ),
              const RoomTabs(),
              Expanded(
                child: TabBarView(
                  children: [
                    chat,
                    Column(
                      children: [
                        if (_controller.issues.isNotEmpty) _issuesBanner(),
                        if (_controller.skipped.isNotEmpty) _skippedBanner(),
                        // Outcomes lead. The raw board is still here, one tap
                        // down: leading with every record is what made a busy
                        // room unreadable, but hiding it would lose the audit
                        // trail people need when something looks wrong.
                        Expanded(
                          child: _showAllRecords
                              ? Column(
                                  children: [
                                    Align(
                                      alignment: Alignment.centerLeft,
                                      child: TextButton.icon(
                                        onPressed: () => setState(
                                          () => _showAllRecords = false,
                                        ),
                                        icon: const Icon(
                                          Icons.arrow_back,
                                          size: 18,
                                        ),
                                        label: const Text('Back to objectives'),
                                      ),
                                    ),
                                    Expanded(
                                      child: ActivityRail(
                                        store: store,
                                        showBoard: true,
                                        canAnswer: _controller.isRunning,
                                      ),
                                    ),
                                  ],
                                )
                              : ObjectivesPane(
                                  store: store,
                                  onShowAllRecords: () =>
                                      setState(() => _showAllRecords = true),
                                ),
                        ),
                      ],
                    ),
                    WorkspaceFilesScreen(
                      config: canRunAgentClis
                          ? WorkspaceConfig(
                              path: Directory(
                                expandHome(widget.room.path),
                              ).absolute.path,
                            )
                          : widget.room.workspace,
                      roomId: widget.room.id,
                      embedded: true,
                    ),
                    _roomMemory(store),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _roomMemory(RoomStore store) {
    final scheme = Theme.of(context).colorScheme;
    final messages = store.messages;
    final tasks = store.tasks;
    final artifacts = store.artifacts;
    final header = <Widget>[
      Text('Room memory', style: Theme.of(context).textTheme.titleLarge),
      const SizedBox(height: 6),
      Text(
        '${messages.length} messages · ${tasks.length} tasks · ${artifacts.length} artifacts saved with this room.',
        style: TextStyle(color: scheme.onSurfaceVariant),
      ),
      if (widget.room.goal.isNotEmpty) ...[
        const SizedBox(height: 24),
        Text('Goal', style: Theme.of(context).textTheme.labelLarge),
        const SizedBox(height: 6),
        SelectableText(widget.room.goal),
      ],
      const SizedBox(height: 24),
      TextField(
        controller: _memory,
        minLines: 4,
        maxLines: 10,
        maxLength: 6000,
        decoration: const InputDecoration(
          labelText: 'Pinned notes',
          alignLabelWithHint: true,
          hintText:
              'Decisions, context and preferences the team should retain.',
        ),
      ),
      Align(
        alignment: Alignment.centerRight,
        child: FilledButton(
          onPressed: _savingMemory
              ? null
              : () async {
                  setState(() => _savingMemory = true);
                  widget.room.memory = _memory.text.trim();
                  await widget.library.save();
                  if (mounted) {
                    setState(() => _savingMemory = false);
                    if (widget.library.error == null) {
                      if (_controller.isRunning) {
                        _controller.say(
                          widget.library.userName,
                          'Update the pinned memory for this room: ${widget.room.memory.isEmpty ? '(No pinned notes.)' : widget.room.memory}',
                        );
                      }
                      ScaffoldMessenger.of(context).showSnackBar(
                        const SnackBar(content: Text('Room notes saved')),
                      );
                    }
                  }
                },
          child: Text(_savingMemory ? 'Saving…' : 'Save notes'),
        ),
      ),
      const SizedBox(height: 24),
      Text('Saved artifacts', style: Theme.of(context).textTheme.titleMedium),
      const SizedBox(height: 8),
      if (artifacts.isEmpty)
        Text(
          'Reports and other outputs will appear here.',
          style: TextStyle(color: scheme.onSurfaceVariant),
        ),
    ];
    return ListView.builder(
      padding: const EdgeInsets.all(20),
      itemCount: header.length + artifacts.length,
      itemBuilder: (context, index) {
        if (index < header.length) return header[index];
        final artifact = artifacts[index - header.length];
        return ListTile(
          contentPadding: EdgeInsets.zero,
          leading: const Icon(Icons.description_outlined),
          title: Text(artifact['name'] as String? ?? 'Artifact'),
          subtitle: Text('@${store.handleFor(artifact['author'] as String?)}'),
          trailing: const Icon(Icons.chevron_right),
          onTap: () {
            final body = artifact['body'] as Map? ?? {};
            final text =
                (body['text'] ??
                        body['diff'] ??
                        body['url'] ??
                        body['path'] ??
                        const JsonEncoder.withIndent(
                          '  ',
                        ).convert(body['value']))
                    .toString();
            Navigator.push(
              context,
              MaterialPageRoute(
                builder: (_) => Scaffold(
                  appBar: AppBar(
                    title: Text(artifact['name'] as String? ?? 'Artifact'),
                  ),
                  body: SelectionArea(
                    child: SingleChildScrollView(
                      padding: const EdgeInsets.all(20),
                      child: MessageText(text: text),
                    ),
                  ),
                ),
              ),
            );
          },
        );
      },
    );
  }

  /// Why this room may never deliver anything.
  Widget _issuesBanner() {
    final scheme = Theme.of(context).colorScheme;
    final blocking = hasBlockingIssue(_controller.issues);
    final tone = blocking ? scheme.error : Colors.orange;
    return Container(
      width: double.infinity,
      color: tone.withValues(alpha: 0.12),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxHeight: 220),
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Icon(
                    blocking ? Icons.error_outline : Icons.info_outline,
                    size: 15,
                    color: tone,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      blocking
                          ? 'This room cannot finish its work'
                          : 'Worth knowing about this room',
                      style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w700,
                        color: tone,
                      ),
                    ),
                  ),
                ],
              ),
              for (final issue in _controller.issues) ...[
                const SizedBox(height: 5),
                Text(
                  '${issue.summary}  ${issue.fix}',
                  style: TextStyle(
                    fontSize: 11,
                    height: 1.35,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  Widget _skippedBanner() {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      width: double.infinity,
      color: Colors.orange.withValues(alpha: 0.15),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      child: Row(
        children: [
          const Icon(Icons.info_outline, size: 16, color: Colors.orange),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              'Left out: ' + _controller.skipped.join('; '),
              style: TextStyle(fontSize: 12, color: scheme.onSurface),
            ),
          ),
        ],
      ),
    );
  }
}
