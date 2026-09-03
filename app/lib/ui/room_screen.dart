/// The room: chat, roster, and board, live.
///
/// Responsive by width. On a wide window (desktop) the three panes sit
/// side by side; on a narrow one (phone) they become tabs, with the chat
/// front and centre.
library;

import 'package:flutter/material.dart';

import '../net/room_client.dart';
import '../protocol/models.dart';
import '../state/room_store.dart';
import 'widgets/board_pane.dart';
import 'widgets/chat_pane.dart';
import 'widgets/roster_pane.dart';

/// The main room view.
class RoomScreen extends StatelessWidget {
  const RoomScreen({
    super.key,
    required this.store,
    required this.client,
    required this.displayName,
  });

  final RoomStore store;
  final RoomClient client;
  final String displayName;

  /// The width below which the layout collapses to tabs.
  static const double _wideBreakpoint = 900;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: store,
      builder: (context, _) {
        final wide = MediaQuery.sizeOf(context).width >= _wideBreakpoint;
        return Scaffold(
          appBar: _buildAppBar(context),
          body: store.connection == LinkState.connecting && store.room == null
              ? const Center(child: CircularProgressIndicator())
              : wide
                  ? _wideLayout()
                  : _narrowLayout(),
        );
      },
    );
  }

  PreferredSizeWidget _buildAppBar(BuildContext context) {
    final room = store.room;
    final scheme = Theme.of(context).colorScheme;
    return AppBar(
      title: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(room?.name ?? 'Connecting…', style: const TextStyle(fontSize: 18)),
          if (room != null)
            Text(
              room.goal,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
            ),
        ],
      ),
      actions: [
        _ConnectionDot(state: store.connection),
        if (room != null) ...[
          const SizedBox(width: 8),
          _PhaseChip(phase: room.phase),
          IconButton(
            tooltip: store.isPaused ? 'Resume' : 'Pause',
            icon: Icon(store.isPaused ? Icons.play_arrow : Icons.pause),
            onPressed: room.phase.isFinished ? null : store.togglePause,
          ),
        ],
        const SizedBox(width: 8),
      ],
    );
  }

  Widget _wideLayout() {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(width: 260, child: RosterPane(store: store)),
        const VerticalDivider(width: 1),
        Expanded(child: ChatPane(store: store, displayName: displayName)),
        const VerticalDivider(width: 1),
        SizedBox(width: 340, child: BoardPane(store: store)),
      ],
    );
  }

  Widget _narrowLayout() {
    return DefaultTabController(
      length: 3,
      child: Column(
        children: [
          const TabBar(
            tabs: [
              Tab(icon: Icon(Icons.chat_bubble_outline), text: 'Chat'),
              Tab(icon: Icon(Icons.checklist), text: 'Board'),
              Tab(icon: Icon(Icons.group_outlined), text: 'Bots'),
            ],
          ),
          Expanded(
            child: TabBarView(
              children: [
                ChatPane(store: store, displayName: displayName),
                BoardPane(store: store),
                RosterPane(store: store),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A small coloured dot showing the connection state.
class _ConnectionDot extends StatelessWidget {
  const _ConnectionDot({required this.state});

  final LinkState state;

  @override
  Widget build(BuildContext context) {
    final (color, label) = switch (state) {
      LinkState.connected => (Colors.green, 'connected'),
      LinkState.connecting => (Colors.amber, 'connecting'),
      LinkState.closed => (Colors.grey, 'closed'),
      LinkState.error => (Colors.red, 'error'),
      LinkState.disconnected => (Colors.grey, 'offline'),
    };
    return Tooltip(
      message: label,
      child: Container(
        width: 10,
        height: 10,
        decoration: BoxDecoration(color: color, shape: BoxShape.circle),
      ),
    );
  }
}

/// A chip showing the room phase.
class _PhaseChip extends StatelessWidget {
  const _PhaseChip({required this.phase});

  final RoomPhase phase;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Chip(
        label: Text(phase.name, style: const TextStyle(fontSize: 11)),
        visualDensity: VisualDensity.compact,
        backgroundColor: phase.isLive
            ? scheme.primaryContainer
            : scheme.surfaceContainerHighest,
        side: BorderSide.none,
      ),
    );
  }
}
