// The pieces a room is made of: the activity rail, the transcript, the board
// strip and the composer.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'package:flutter/material.dart';

import '../protocol/models.dart';
import '../state/room_store.dart';
import 'brand.dart';

/// Who is doing what, right now.
///
/// This is the answer to "I want to see what the model is doing, not in the
/// chat space". Each bot gets a row: its status, the task it holds, and the
/// last thing it actually did.
class ActivityRail extends StatelessWidget {
  const ActivityRail({super.key, required this.store});

  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final activity = store.activity;
    return Container(
      color: scheme.surfaceContainerLow,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _header(context, activity.length),
          Expanded(
            child: ListView.builder(
              padding: const EdgeInsets.fromLTRB(10, 10, 10, 16),
              itemCount: activity.length,
              itemBuilder: (context, i) => _BotRow(entry: activity[i]),
            ),
          ),
          _footer(context),
        ],
      ),
    );
  }

  Widget _header(BuildContext context, int count) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
      ),
      child: Row(
        children: [
          Icon(Icons.insights_outlined, size: 17, color: scheme.onSurfaceVariant),
          const SizedBox(width: 9),
          const Text('Activity',
              style: TextStyle(fontWeight: FontWeight.w700, fontSize: 14)),
          const Spacer(),
          Text('$count',
              style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant)),
        ],
      ),
    );
  }

  /// A one-line summary of the board, so progress is visible at a glance.
  Widget _footer(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final tasks = store.tasks;
    if (tasks.isEmpty) return const SizedBox.shrink();
    final done = tasks.where((t) => t.state == TaskState.submitted).length;
    final active = tasks.where((t) => t.state.isActive).length;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: scheme.outlineVariant)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text('$done of ${tasks.length} delivered',
                  style: const TextStyle(
                      fontSize: 12, fontWeight: FontWeight.w600)),
              const Spacer(),
              if (active > 0)
                Text('$active in flight',
                    style: TextStyle(
                        fontSize: 11, color: scheme.onSurfaceVariant)),
            ],
          ),
          const SizedBox(height: 8),
          ClipRRect(
            borderRadius: BorderRadius.circular(4),
            child: LinearProgressIndicator(
              value: tasks.isEmpty ? 0 : done / tasks.length,
              minHeight: 5,
              backgroundColor: scheme.surfaceContainerHighest,
            ),
          ),
        ],
      ),
    );
  }
}

class _BotRow extends StatelessWidget {
  const _BotRow({required this.entry});

  final BotActivity entry;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (color, label) = _status(entry.agent.status, entry.streaming);

    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(
          // A working bot is outlined so the eye finds it immediately.
          color: entry.streaming || entry.agent.status == AgentStatus.working
              ? color.withValues(alpha: 0.5)
              : scheme.outlineVariant,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              BotAvatar(handle: entry.agent.handle, radius: 14),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('@${entry.agent.handle}',
                        style: const TextStyle(
                            fontWeight: FontWeight.w700, fontSize: 13)),
                    if (entry.agent.roles.isNotEmpty)
                      Text(entry.agent.roles.join(', '),
                          style: TextStyle(
                              fontSize: 11, color: scheme.onSurfaceVariant)),
                  ],
                ),
              ),
              StatusPill(color: color, label: label),
            ],
          ),
          if (entry.taskTitle != null) ...[
            const SizedBox(height: 10),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(Icons.subdirectory_arrow_right,
                    size: 13, color: scheme.onSurfaceVariant),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(entry.taskTitle!,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 12)),
                ),
              ],
            ),
          ],
          if (entry.lastAction != null) ...[
            const SizedBox(height: 8),
            Text(
              entry.lastAction! + _ago(entry.lastActionAt),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 11, height: 1.3, color: scheme.onSurfaceVariant),
            ),
          ],
          if (entry.tasksSubmitted + entry.reviewsGiven + entry.handoffsSent > 0) ...[
            const SizedBox(height: 8),
            Wrap(
              spacing: 12,
              children: [
                if (entry.tasksSubmitted > 0)
                  _Tally(icon: Icons.check_circle_outline, n: entry.tasksSubmitted),
                if (entry.reviewsGiven > 0)
                  _Tally(icon: Icons.rate_review_outlined, n: entry.reviewsGiven),
                if (entry.handoffsSent > 0)
                  _Tally(icon: Icons.swap_horiz, n: entry.handoffsSent),
              ],
            ),
          ],
        ],
      ),
    );
  }

  static (Color, String) _status(AgentStatus status, bool streaming) {
    if (streaming) return (Colors.lightBlue, 'thinking');
    switch (status) {
      case AgentStatus.working:
        return (Colors.blue, 'working');
      case AgentStatus.waiting:
        return (Colors.amber, 'waiting');
      case AgentStatus.idle:
        return (Colors.green, 'ready');
      case AgentStatus.faulted:
        return (Colors.red, 'faulted');
      case AgentStatus.offline:
      case AgentStatus.unknown:
        return (Colors.grey, 'offline');
    }
  }

  static String _ago(DateTime? at) {
    if (at == null) return '';
    final seconds = DateTime.now().difference(at).inSeconds;
    if (seconds < 5) return ' · just now';
    if (seconds < 60) return ' · ${seconds}s ago';
    return ' · ${(seconds / 60).floor()}m ago';
  }
}

class _Tally extends StatelessWidget {
  const _Tally({required this.icon, required this.n});
  final IconData icon;
  final int n;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 12, color: scheme.onSurfaceVariant),
        const SizedBox(width: 3),
        Text('$n',
            style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant)),
      ],
    );
  }
}

/// The room transcript.
class Transcript extends StatelessWidget {
  const Transcript({super.key, required this.store, required this.controller});

  final RoomStore store;
  final ScrollController controller;

  @override
  Widget build(BuildContext context) {
    final messages = store.messages;
    if (messages.isEmpty) {
      return const EmptyState(
        icon: Icons.forum_outlined,
        title: 'The room is quiet',
        message: 'The bots are getting started. Anything you say goes to all '
            'of them — watch the activity panel to see what they are doing.',
      );
    }
    // SelectionArea rather than a SelectableText per bubble: it selects across
    // the whole conversation, which is what you want when copying one out, and
    // it avoids EditableText, which never settles inside a widget test.
    return SelectionArea(
      child: ListView.builder(
        controller: controller,
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        itemCount: messages.length,
        itemBuilder: (context, i) => _Bubble(message: messages[i], store: store),
      ),
    );
  }
}

class _Bubble extends StatelessWidget {
  const _Bubble({required this.message, required this.store});

  final ChatMessage message;
  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final author = message.author;

    if (author.isSystem) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 7),
        child: Center(
          child: Text(message.body,
              textAlign: TextAlign.center,
              style: TextStyle(
                  fontSize: 11,
                  fontStyle: FontStyle.italic,
                  color: scheme.onSurfaceVariant)),
        ),
      );
    }

    final isHuman = author.isHuman;
    final name = isHuman ? (author.name ?? 'you') : store.handleFor(author.agentId);
    final color = isHuman ? scheme.primary : handleColor(name);

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (isHuman)
            CircleAvatar(
              radius: 15,
              backgroundColor: color.withValues(alpha: 0.18),
              child: Icon(Icons.person, size: 16, color: color),
            )
          else
            BotAvatar(handle: name, radius: 15),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Text(isHuman ? name : '@$name',
                        style: TextStyle(
                            fontWeight: FontWeight.w700,
                            fontSize: 12.5,
                            color: color)),
                    if (message.intent == 'ask') ...[
                      const SizedBox(width: 6),
                      Icon(Icons.help_outline,
                          size: 12, color: scheme.onSurfaceVariant),
                    ],
                  ],
                ),
                const SizedBox(height: 3),
                Text(message.body,
                    style: const TextStyle(fontSize: 14, height: 1.4)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A compact board across the top of the chat.
class BoardStrip extends StatelessWidget {
  const BoardStrip({super.key, required this.store});

  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final tasks = store.tasks;
    return Container(
      constraints: const BoxConstraints(maxHeight: 210),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
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
                for (final task in tasks)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 8),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        StatusPill(
                            color: taskColor(task.state), label: task.state.label),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(task.title,
                                  style: const TextStyle(
                                      fontSize: 13,
                                      fontWeight: FontWeight.w600)),
                              if (task.assignee != null)
                                Text('@' + store.handleFor(task.assignee),
                                    style: TextStyle(
                                        fontSize: 11,
                                        color: scheme.onSurfaceVariant)),
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
}

/// The colour a task state reads as.
Color taskColor(TaskState state) {
  switch (state) {
    case TaskState.claimed:
    case TaskState.inProgress:
      return Colors.blue;
    case TaskState.blocked:
      return Colors.amber;
    case TaskState.inReview:
      return Colors.purple;
    case TaskState.approved:
      return Colors.teal;
    case TaskState.submitted:
      return Colors.green;
    case TaskState.failed:
    case TaskState.cancelled:
      return Colors.red;
    default:
      return Colors.blueGrey;
  }
}

/// The message input.
class Composer extends StatelessWidget {
  const Composer({super.key, required this.controller, required this.onSend});

  final TextEditingController controller;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 14),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Expanded(
            child: TextField(
              controller: controller,
              minLines: 1,
              maxLines: 5,
              textInputAction: TextInputAction.send,
              onSubmitted: (_) => onSend(),
              decoration: InputDecoration(
                hintText: 'Message the room…',
                filled: true,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(24),
                  borderSide: BorderSide.none,
                ),
                contentPadding:
                    const EdgeInsets.symmetric(horizontal: 18, vertical: 13),
              ),
            ),
          ),
          const SizedBox(width: 9),
          FilledButton(
            onPressed: onSend,
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
