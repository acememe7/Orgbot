/// The roster: who is in the room and what each bot is doing.
library;

import 'package:flutter/material.dart';

import '../../protocol/models.dart';
import '../../state/room_store.dart';

/// The left pane: the bots, with live status.
class RosterPane extends StatelessWidget {
  const RosterPane({super.key, required this.store});

  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final agents = store.agents;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const _PaneHeader(icon: Icons.group_outlined, title: 'Bots'),
        Expanded(
          child: ListView.builder(
            padding: const EdgeInsets.symmetric(vertical: 4),
            itemCount: agents.length,
            itemBuilder: (context, i) => _AgentTile(agent: agents[i]),
          ),
        ),
        if (store.openHandoffs.isNotEmpty) _HandoffStrip(store: store),
      ],
    );
  }
}

class _AgentTile extends StatelessWidget {
  const _AgentTile({required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (statusColor, statusLabel) = _status(agent.status);
    return ListTile(
      dense: true,
      leading: Stack(
        children: [
          CircleAvatar(
            radius: 16,
            backgroundColor: scheme.primaryContainer,
            child: Text(
              agent.handle.isEmpty ? '?' : agent.handle.substring(0, 1).toUpperCase(),
              style: TextStyle(fontSize: 12, color: scheme.onPrimaryContainer),
            ),
          ),
          Positioned(
            right: 0,
            bottom: 0,
            child: Container(
              width: 10,
              height: 10,
              decoration: BoxDecoration(
                color: statusColor,
                shape: BoxShape.circle,
                border: Border.all(color: scheme.surface, width: 1.5),
              ),
            ),
          ),
        ],
      ),
      title: Text('@${agent.handle}', style: const TextStyle(fontWeight: FontWeight.w600)),
      subtitle: Text(
        agent.roles.isEmpty ? statusLabel : '${agent.roles.join(', ')} · $statusLabel',
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: const TextStyle(fontSize: 11),
      ),
    );
  }

  static (Color, String) _status(AgentStatus status) {
    switch (status) {
      case AgentStatus.idle:
        return (Colors.green, 'idle');
      case AgentStatus.working:
        return (Colors.blue, 'working');
      case AgentStatus.waiting:
        return (Colors.amber, 'waiting');
      case AgentStatus.offline:
        return (Colors.grey, 'offline');
      case AgentStatus.faulted:
        return (Colors.red, 'faulted');
      case AgentStatus.unknown:
        return (Colors.grey, 'unknown');
    }
  }
}

/// A compact strip of in-flight handoffs at the bottom of the roster.
class _HandoffStrip extends StatelessWidget {
  const _HandoffStrip({required this.store});

  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final handoffs = store.openHandoffs;
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        border: Border(top: BorderSide(color: scheme.outlineVariant)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.swap_horiz, size: 14, color: scheme.onSurfaceVariant),
              const SizedBox(width: 6),
              Text(
                '${handoffs.length} handoff${handoffs.length == 1 ? '' : 's'} in flight',
                style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
              ),
            ],
          ),
          const SizedBox(height: 6),
          for (final handoff in handoffs.take(3))
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                '@${store.handleFor(handoff.from)} → ${handoff.kind}: ${handoff.brief}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: 11),
              ),
            ),
        ],
      ),
    );
  }
}

/// A small section header used by the side panes.
class _PaneHeader extends StatelessWidget {
  const _PaneHeader({required this.icon, required this.title});

  final IconData icon;
  final String title;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
      ),
      child: Row(
        children: [
          Icon(icon, size: 18, color: scheme.onSurfaceVariant),
          const SizedBox(width: 8),
          Text(title, style: const TextStyle(fontWeight: FontWeight.w600)),
        ],
      ),
    );
  }
}
