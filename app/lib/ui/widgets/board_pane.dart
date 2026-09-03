/// The task board: every task, grouped by how far along it is.
library;

import 'package:flutter/material.dart';

import '../../protocol/models.dart';
import '../../state/room_store.dart';

/// The right pane: the shared task board.
class BoardPane extends StatelessWidget {
  const BoardPane({super.key, required this.store});

  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final tasks = store.tasks;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _header(context, tasks.length),
        Expanded(
          child: tasks.isEmpty
              ? const Center(
                  child: Padding(
                    padding: EdgeInsets.all(24),
                    child: Text(
                      'The board is empty.\nA coordinator will fill it in, or seed tasks in the room file.',
                      textAlign: TextAlign.center,
                    ),
                  ),
                )
              : ListView.builder(
                  padding: const EdgeInsets.all(12),
                  itemCount: tasks.length,
                  itemBuilder: (context, i) => _TaskCard(task: tasks[i], store: store),
                ),
        ),
      ],
    );
  }

  Widget _header(BuildContext context, int count) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
      ),
      child: Row(
        children: [
          Icon(Icons.checklist, size: 18, color: scheme.onSurfaceVariant),
          const SizedBox(width: 8),
          const Text('Board', style: TextStyle(fontWeight: FontWeight.w600)),
          const Spacer(),
          Text('$count', style: TextStyle(color: scheme.onSurfaceVariant)),
        ],
      ),
    );
  }
}

class _TaskCard extends StatelessWidget {
  const _TaskCard({required this.task, required this.store});

  final Task task;
  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (color, label) = _stateStyle(task.state);
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      elevation: 0,
      color: scheme.surfaceContainerHighest,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(10),
        side: BorderSide(color: color.withValues(alpha: 0.4)),
      ),
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Container(
                  padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                  decoration: BoxDecoration(
                    color: color.withValues(alpha: 0.18),
                    borderRadius: BorderRadius.circular(6),
                  ),
                  child: Text(
                    label,
                    style: TextStyle(fontSize: 10, fontWeight: FontWeight.w700, color: color),
                  ),
                ),
                const Spacer(),
                if (task.priority == Priority.high || task.priority == Priority.urgent)
                  Icon(
                    Icons.priority_high,
                    size: 14,
                    color: task.priority == Priority.urgent ? Colors.red : Colors.orange,
                  ),
              ],
            ),
            const SizedBox(height: 8),
            Text(task.title, style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13)),
            if (task.brief.isNotEmpty) ...[
              const SizedBox(height: 4),
              Text(
                task.brief,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
              ),
            ],
            const SizedBox(height: 8),
            Row(
              children: [
                Icon(Icons.person_outline, size: 13, color: scheme.onSurfaceVariant),
                const SizedBox(width: 4),
                Text(
                  task.assignee == null ? 'unassigned' : '@${store.handleFor(task.assignee)}',
                  style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
                ),
                if (task.revision > 0) ...[
                  const SizedBox(width: 10),
                  Icon(Icons.replay, size: 13, color: scheme.onSurfaceVariant),
                  const SizedBox(width: 2),
                  Text('rev ${task.revision}',
                      style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant)),
                ],
                if (task.blockedCount > 0) ...[
                  const SizedBox(width: 10),
                  Icon(Icons.block, size: 13, color: Colors.amber),
                  const SizedBox(width: 2),
                  Text('${task.blockedCount}',
                      style: const TextStyle(fontSize: 11, color: Colors.amber)),
                ],
              ],
            ),
          ],
        ),
      ),
    );
  }

  static (Color, String) _stateStyle(TaskState state) {
    switch (state) {
      case TaskState.draft:
        return (Colors.grey, 'DRAFT');
      case TaskState.ready:
        return (Colors.blueGrey, 'READY');
      case TaskState.claimed:
        return (Colors.lightBlue, 'CLAIMED');
      case TaskState.inProgress:
        return (Colors.blue, 'IN PROGRESS');
      case TaskState.blocked:
        return (Colors.amber, 'BLOCKED');
      case TaskState.inReview:
        return (Colors.purple, 'IN REVIEW');
      case TaskState.approved:
        return (Colors.teal, 'APPROVED');
      case TaskState.submitted:
        return (Colors.green, 'SUBMITTED');
      case TaskState.failed:
        return (Colors.red, 'FAILED');
      case TaskState.cancelled:
        return (Colors.grey, 'CANCELLED');
      case TaskState.unknown:
        return (Colors.grey, 'UNKNOWN');
    }
  }
}
