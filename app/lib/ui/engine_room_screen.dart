// A room, running on the real engine.
//
// Three regions, because a room has three questions: what is being said
// (chat), who is doing what right now (the activity rail), and what the work
// actually is (the board). The activity rail is the one that was missing —
// most of what a room does is work, not talk, and none of it was visible.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'package:flutter/material.dart';

import '../orgbot/library.dart';
import '../orgbot/models.dart' as defs;
import '../protocol/models.dart';
import '../state/engine_room_controller.dart';
import '../orgbot/room_check.dart';
import '../state/room_store.dart';
import 'brand.dart';
import 'room_widgets.dart';

/// Runs a saved room on the engine and shows it.
class EngineRoomScreen extends StatefulWidget {
  const EngineRoomScreen({
    super.key,
    required this.room,
    required this.library,
    this.displayName = 'you',
  });

  final defs.RoomDef room;
  final Library library;
  final String displayName;

  @override
  State<EngineRoomScreen> createState() => _EngineRoomScreenState();
}

class _EngineRoomScreenState extends State<EngineRoomScreen> {
  late final EngineRoomController _controller;
  final TextEditingController _input = TextEditingController();
  final ScrollController _scroll = ScrollController();
  bool _showBoard = false;
  int _seen = 0;

  /// Below this the rail becomes a tab rather than a column.
  static const double _wide = 1000;

  @override
  void initState() {
    super.initState();
    _controller =
        EngineRoomController(room: widget.room, library: widget.library);
    _controller.addListener(_onChange);
    _controller.start();
  }

  void _onChange() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _controller.removeListener(_onChange);
    _controller.dispose();
    _input.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _send() {
    final text = _input.text.trim();
    if (text.isEmpty) return;
    _input.clear();
    _controller.say(widget.displayName, text);
  }

  void _follow(int count) {
    if (count == _seen) return;
    _seen = count;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.animateTo(_scroll.position.maxScrollExtent,
            duration: const Duration(milliseconds: 220), curve: Curves.easeOut);
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final store = _controller.store;
    final wide = MediaQuery.sizeOf(context).width >= _wide;

    return Scaffold(
      appBar: _appBar(store),
      body: switch (_controller.phase) {
        RoomLaunch.failed => _failure(),
        RoomLaunch.running when store != null => _room(store, wide),
        _ => _starting(),
      },
    );
  }

  PreferredSizeWidget _appBar(RoomStore? store) {
    final scheme = Theme.of(context).colorScheme;
    return AppBar(
      titleSpacing: 8,
      title: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(widget.room.name, style: const TextStyle(fontSize: 17)),
          Text(
            widget.room.goal.isEmpty ? _controller.statusLine : widget.room.goal,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
          ),
        ],
      ),
      actions: [
        if (_controller.isRunning) ...[
          _phasePill(store),
          IconButton(
            tooltip: _showBoard ? 'Hide the board' : 'Show the board',
            icon: Icon(
                _showBoard ? Icons.checklist : Icons.checklist_outlined),
            onPressed: () => setState(() => _showBoard = !_showBoard),
          ),
          IconButton(
            tooltip: store?.isPaused == true ? 'Resume' : 'Pause',
            icon: Icon(store?.isPaused == true ? Icons.play_arrow : Icons.pause),
            onPressed: _controller.togglePause,
          ),
        ],
        const SizedBox(width: 6),
      ],
    );
  }

  Widget _phasePill(RoomStore? store) {
    final phase = store?.room?.phase ?? RoomPhase.running;
    final (color, label) = switch (phase) {
      RoomPhase.running => (Colors.green, 'live'),
      RoomPhase.paused => (Colors.amber, 'paused'),
      RoomPhase.completed => (Colors.blueGrey, 'done'),
      RoomPhase.aborted => (Colors.red, 'stopped'),
      _ => (Colors.grey, phase.name),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Center(child: StatusPill(color: color, label: label)),
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
                              : Icon(Icons.circle_outlined,
                                  size: 14, color: scheme.outlineVariant),
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
              Text('The room could not start',
                  style: Theme.of(context).textTheme.titleMedium),
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
    _follow(store.messages.length);
    final chat = Column(
      children: [
        if (_controller.issues.isNotEmpty) _issuesBanner(),
        if (_controller.skipped.isNotEmpty) _skippedBanner(),
        if (_showBoard) BoardStrip(store: store),
        Expanded(child: Transcript(store: store, controller: _scroll)),
        const Divider(height: 1),
        Composer(controller: _input, onSend: _send),
      ],
    );

    if (!wide) {
      return DefaultTabController(
        length: 2,
        child: Column(
          children: [
            const TabBar(tabs: [
              Tab(icon: Icon(Icons.chat_bubble_outline), text: 'Chat'),
              Tab(icon: Icon(Icons.insights_outlined), text: 'Activity'),
            ]),
            Expanded(
              child: TabBarView(children: [chat, ActivityRail(store: store)]),
            ),
          ],
        ),
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(child: chat),
        const VerticalDivider(width: 1),
        SizedBox(width: 320, child: ActivityRail(store: store)),
      ],
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
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(blocking ? Icons.error_outline : Icons.info_outline,
                  size: 15, color: tone),
              const SizedBox(width: 8),
              Text(
                blocking
                    ? 'This room cannot finish its work'
                    : 'Worth knowing about this room',
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w700, color: tone),
              ),
            ],
          ),
          for (final issue in _controller.issues) ...[
            const SizedBox(height: 5),
            Text('${issue.summary}  ${issue.fix}',
                style: TextStyle(
                    fontSize: 11,
                    height: 1.35,
                    color: scheme.onSurfaceVariant)),
          ],
        ],
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
