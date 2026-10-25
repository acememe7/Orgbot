// The pieces a room is made of: the activity rail, the transcript, the board
// strip and the composer.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'dart:io';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:url_launcher/url_launcher.dart';

import '../protocol/models.dart';
import '../state/room_store.dart';
import 'brand.dart';

/// Who is doing what, right now.
///
/// This is the answer to "I want to see what the model is doing, not in the
/// chat space". Each bot gets a row: its status, the task it holds, and the
/// last thing it actually did.
class ActivityRail extends StatelessWidget {
  const ActivityRail({
    super.key,
    required this.store,
    this.showBoard = false,
    this.canAnswer = true,
  });

  final bool showBoard, canAnswer;

  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final activity = store.activity;
    return Container(
      color: scheme.surface,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: ListView.builder(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
              itemCount: activity.length + (showBoard ? 2 : 1),
              itemBuilder: (context, i) {
                if (showBoard && i == 0) {
                  return BoardStrip(
                    store: store,
                    canAnswer: canAnswer,
                    embedded: true,
                  );
                }
                final row = i - (showBoard ? 2 : 1);
                if (row < 0) return _header(context, activity.length);
                return _BotRow(entry: activity[row]);
              },
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
          Icon(
            Icons.insights_outlined,
            size: 17,
            color: scheme.onSurfaceVariant,
          ),
          const SizedBox(width: 9),
          const Text(
            'Activity',
            style: TextStyle(fontWeight: FontWeight.w700, fontSize: 14),
          ),
          const Spacer(),
          Text(
            '$count',
            style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
          ),
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
          Wrap(
            alignment: WrapAlignment.spaceBetween,
            spacing: 16,
            runSpacing: 4,
            children: [
              Text(
                '$done of ${tasks.length} delivered',
                style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
              if (active > 0)
                Text(
                  '$active in flight',
                  style: TextStyle(
                    fontSize: 11,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
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
    final activeStep = entry.activeStep;
    final (color, label) = entry.streaming && entry.retryStatus != null
        ? (Colors.amber, entry.retryStatus!)
        : entry.streaming && activeStep != null
        ? (Colors.lightBlue, activeStep.label)
        : _status(entry.agent.status, entry.streaming, entry.lastError != null);

    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(18),
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
                    Text(
                      entry.agent.displayName.isEmpty
                          ? '@${entry.agent.handle}'
                          : entry.agent.displayName,
                      style: const TextStyle(
                        fontWeight: FontWeight.w700,
                        fontSize: 13,
                      ),
                    ),
                    if (entry.agent.roles.isNotEmpty)
                      Text(
                        entry.agent.roles
                            .map((r) => r.replaceAll('_', ' '))
                            .join(', '),
                        style: TextStyle(
                          fontSize: 11,
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                ),
              ),
              Flexible(
                child: StatusPill(color: color, label: label),
              ),
            ],
          ),
          if (entry.taskTitle != null) ...[
            const SizedBox(height: 10),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.subdirectory_arrow_right,
                  size: 13,
                  color: scheme.onSurfaceVariant,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    entry.taskTitle!,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 12),
                  ),
                ),
              ],
            ),
          ],
          if (entry.lastError != null) ...[
            const SizedBox(height: 9),
            Text(
              'Previous turn failed${_ago(entry.lastErrorAt)}',
              style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
            ),
            const SizedBox(height: 4),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(8),
              decoration: BoxDecoration(
                color: scheme.errorContainer.withValues(alpha: 0.5),
                borderRadius: BorderRadius.circular(7),
              ),
              child: Text(
                entry.errorStreak > 1
                    ? '${entry.lastError} (${entry.errorStreak} in a row)'
                    : entry.lastError!,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 10.5,
                  height: 1.35,
                  color: scheme.onErrorContainer,
                ),
              ),
            ),
          ],
          if (entry.lastError != null)
            TextButton(
              onPressed: () => showModalBottomSheet<void>(
                context: context,
                isScrollControlled: true,
                showDragHandle: true,
                builder: (context) => SafeArea(
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
                    child: SingleChildScrollView(
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            '@${entry.agent.handle} · Error details',
                            style: Theme.of(context).textTheme.titleMedium,
                          ),
                          const SizedBox(height: 12),
                          SelectableText(entry.lastError!),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
              child: const Text('View error details'),
            ),
          if (entry.lastAction != null &&
              !entry.lastAction!.startsWith('turn failed:')) ...[
            const SizedBox(height: 8),
            Text(
              entry.lastAction! + _ago(entry.lastActionAt),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 11,
                height: 1.3,
                color: scheme.onSurfaceVariant,
              ),
            ),
          ],
          if (entry.steps.isNotEmpty) ...[
            const SizedBox(height: 4),
            Theme(
              data: Theme.of(context).copyWith(
                dividerColor: Colors.transparent,
                visualDensity: VisualDensity.compact,
              ),
              child: Material(
                color: Colors.transparent,
                child: ExpansionTile(
                  key: PageStorageKey<String>('process-${entry.agent.id}'),
                  tilePadding: EdgeInsets.zero,
                  childrenPadding: const EdgeInsets.only(bottom: 4),
                  minTileHeight: 38,
                  title: Text(
                    activeStep == null
                        ? 'Process · ${entry.steps.length} recent steps'
                        : 'Process · ${activeStep.label}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                      color: scheme.onSurfaceVariant,
                    ),
                  ),
                  children: [
                    for (final step in entry.steps.reversed)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 3),
                        child: Row(
                          children: [
                            Icon(
                              step.running
                                  ? Icons.sync
                                  : step.succeeded == false
                                  ? Icons.error_outline
                                  : step.succeeded == true
                                  ? Icons.check_circle_outline
                                  : Icons.stop_circle_outlined,
                              size: 14,
                              color: step.running
                                  ? Colors.lightBlue
                                  : step.succeeded == false
                                  ? scheme.error
                                  : scheme.onSurfaceVariant,
                            ),
                            const SizedBox(width: 8),
                            Expanded(
                              child: Text(
                                step.label,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(fontSize: 11),
                              ),
                            ),
                            const SizedBox(width: 8),
                            Text(
                              step.running
                                  ? 'running'
                                  : step.succeeded == false
                                  ? 'failed'
                                  : step.succeeded == true
                                  ? 'done'
                                  : 'stopped',
                              style: TextStyle(
                                fontSize: 10,
                                color: scheme.onSurfaceVariant,
                              ),
                            ),
                          ],
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ],
          if (entry.tasksSubmitted + entry.reviewsGiven + entry.handoffsSent >
              0) ...[
            const SizedBox(height: 8),
            Wrap(
              spacing: 12,
              children: [
                if (entry.tasksSubmitted > 0)
                  _Tally(
                    icon: Icons.check_circle_outline,
                    n: entry.tasksSubmitted,
                  ),
                if (entry.reviewsGiven > 0)
                  _Tally(
                    icon: Icons.rate_review_outlined,
                    n: entry.reviewsGiven,
                  ),
                if (entry.handoffsSent > 0)
                  _Tally(icon: Icons.swap_horiz, n: entry.handoffsSent),
              ],
            ),
          ],
        ],
      ),
    );
  }

  static (Color, String) _status(
    AgentStatus status,
    bool streaming,
    bool failing,
  ) {
    if (streaming) return (Colors.lightBlue, 'thinking');
    if (failing) return (Colors.red, 'last turn failed');
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
        Text(
          '$n',
          style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
        ),
      ],
    );
  }
}

/// The room transcript.
class Transcript extends StatefulWidget {
  const Transcript({
    super.key,
    required this.store,
    required this.controller,
    this.desktop = false,
    this.attachmentRoot,
    this.onOpenAttachment,
    this.onReply,
  });

  final RoomStore store;
  final ScrollController controller;
  final bool desktop;
  final String? attachmentRoot;
  final void Function(ChatFile)? onOpenAttachment;
  final ValueChanged<ChatMessage>? onReply;

  @override
  State<Transcript> createState() => _TranscriptState();
}

class _TranscriptState extends State<Transcript>
    with AutomaticKeepAliveClientMixin {
  final _center = UniqueKey();
  String? _boundary;
  List<ChatMessage> _messages = [];
  Map<String, int> _messageIndices = const {};
  bool _following = true;
  bool _interacting = false;
  bool _scheduled = false;

  @override
  bool get wantKeepAlive => true;

  // A fixed boundary lets Flutter grow history upwards and arrivals downwards.
  // Appending never renumbers the mounted history or changes its scroll origin.
  @override
  void initState() {
    super.initState();
    final messages = widget.store.messages;
    final confirmed = messages.where((m) => !widget.store.isPending(m));
    if (!widget.desktop && confirmed.isNotEmpty) {
      _boundary = confirmed.last.id;
    }
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted &&
          _following &&
          !_interacting &&
          widget.controller.hasClients) {
        _latest();
      }
    });
  }

  void _refresh() {
    if (_scheduled) return;
    _scheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _scheduled = false;
      if (!mounted) return;
      setState(() {});
    });
  }

  void _hold() {
    _interacting = true;
    _following = false;
    _refresh();
  }

  bool _scroll(ScrollNotification notice) {
    if (notice.depth != 0) return false;
    if (notice is ScrollStartNotification) _hold();
    if (notice is ScrollEndNotification) {
      _interacting = false;
      _following = notice.metrics.extentAfter <= 24;
      _refresh();
    }
    return false;
  }

  void _latest() {
    setState(() {
      _interacting = false;
      _following = true;
    });
    // No queued animations: dragging can always take over immediately.
    widget.controller.jumpTo(widget.controller.position.maxScrollExtent);
  }

  @override
  Widget build(BuildContext context) {
    super.build(context);
    final store = widget.store;
    // Retain the last complete transcript while a reconnect replays its journal.
    if (!store.isReplaying || _messages.isEmpty) {
      final current = store.messages;
      if (!identical(current, _messages)) {
        _messages = current;
        _messageIndices = {
          for (var i = 0; i < current.length; i++) current[i].id: i,
        };
      }
    }
    final messages = _messages;
    final boundary = messages.indexWhere((m) => m.id == _boundary);
    final split = boundary + 1;
    final composing = store.activity.where((a) => a.streaming).toList();
    final scheme = Theme.of(context).colorScheme;

    SliverChildBuilderDelegate children(
      int count,
      int Function(int) index,
      int? Function(int) childIndex,
    ) {
      return SliverChildBuilderDelegate(
        (context, i) {
          final message = messages[index(i)];
          final bubble = _Bubble(
            key: widget.desktop ? null : ValueKey(message.id),
            message: message,
            store: store,
            desktop: widget.desktop,
            pending: store.isPending(message),
            attachmentRoot: widget.attachmentRoot,
            onOpenAttachment: widget.onOpenAttachment,
            onReply: widget.onReply,
          );
          return widget.desktop
              ? Align(
                  key: ValueKey(message.id),
                  alignment: Alignment.center,
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 920),
                    child: bubble,
                  ),
                )
              : bubble;
        },
        childCount: count,
        findChildIndexCallback: (key) {
          final i = _messageIndices[(key as ValueKey<String>).value];
          return i == null ? null : childIndex(i);
        },
      );
    }

    return Stack(
      children: [
        Positioned.fill(
          child: Listener(
            onPointerDown: (_) => _hold(),
            onPointerUp: (_) {
              _interacting = false;
              if (!widget.controller.position.isScrollingNotifier.value) {
                _following = widget.controller.position.extentAfter <= 24;
                _refresh();
              }
            },
            onPointerCancel: (_) => _interacting = false,
            onPointerSignal: (_) => _hold(),
            child: NotificationListener<ScrollNotification>(
              onNotification: _scroll,
              child: ScrollConfiguration(
                behavior: ScrollConfiguration.of(
                  context,
                ).copyWith(scrollbars: false),
                child: Scrollbar(
                  interactive: true,
                  controller: widget.controller,
                  child: SelectionArea(
                    child: CustomScrollView(
                      controller: widget.controller,
                      center: _center,
                      anchor: widget.desktop ? 0 : 1,
                      physics: _TranscriptPhysics(
                        follow: () => _following && !_interacting,
                      ),
                      slivers: [
                        SliverPadding(
                          padding: EdgeInsets.symmetric(
                            horizontal: widget.desktop ? 24 : 20,
                            vertical: widget.desktop ? 12 : 0,
                          ),
                          sliver: _AnchoredSliverList(
                            hold: () => !_following,
                            delegate: children(
                              split,
                              (i) => split - 1 - i,
                              (i) => i < split ? split - 1 - i : null,
                            ),
                          ),
                        ),
                        SliverPadding(
                          key: _center,
                          padding: EdgeInsets.symmetric(
                            horizontal: widget.desktop ? 24 : 20,
                            vertical: widget.desktop ? 12 : 0,
                          ),
                          sliver: _AnchoredSliverList(
                            hold: () => !_following,
                            delegate: children(
                              messages.length - split,
                              (i) => split + i,
                              (i) => i >= split ? i - split : null,
                            ),
                          ),
                        ),
                        const SliverToBoxAdapter(child: SizedBox(height: 48)),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
        if (messages.isEmpty && composing.isEmpty && store.failing.isEmpty)
          const Positioned.fill(
            child: IgnorePointer(
              child: EmptyState(
                icon: Icons.forum_outlined,
                title: 'Your room is ready',
                message:
                    'Send a message or tag a teammate with @. Conversations are saved here.',
              ),
            ),
          ),
        if (store.failing.isNotEmpty)
          Positioned(
            top: 0,
            left: 0,
            right: 0,
            child: _FailureBanner(failing: store.failing),
          ),
        if (composing.isNotEmpty && _following)
          Positioned(
            left: 20,
            right: 20,
            bottom: 0,
            child: ColoredBox(
              color: scheme.surface,
              child: _Typing(
                handles: composing.map((a) => a.agent.handle).toList(),
                status: composing.length == 1
                    ? composing.single.retryStatus
                    : null,
              ),
            ),
          ),
        if (!_following)
          Positioned(
            bottom: 12,
            left: 0,
            right: 0,
            child: Center(
              child: FilledButton.tonalIcon(
                onPressed: _latest,
                icon: const Icon(Icons.arrow_downward, size: 16),
                label: const Text('Jump to latest'),
              ),
            ),
          ),
      ],
    );
  }
}

// Keep the first visible message fixed when a mounted neighbor changes height.
// Native slivers still own virtualization, measurement and gesture physics;
// their scrollOffsetCorrection applies this adjustment before the frame paints.
class _AnchoredSliverList extends SliverList {
  const _AnchoredSliverList({required super.delegate, required this.hold});
  final bool Function() hold;
  @override
  RenderSliverList createRenderObject(BuildContext context) =>
      _AnchoredListRender(
        childManager: context as SliverMultiBoxAdaptorElement,
        hold: hold,
      );
  @override
  void updateRenderObject(
    BuildContext context,
    _AnchoredListRender renderObject,
  ) {
    renderObject.hold = hold;
  }
}

class _AnchoredListRender extends RenderSliverList {
  _AnchoredListRender({required super.childManager, required this.hold});
  bool Function() hold;
  @override
  void performLayout() {
    RenderBox? anchor;
    double? before;
    final reverse = constraints.growthDirection == GrowthDirection.reverse;
    if (hold()) {
      final start = constraints.scrollOffset;
      final end = start + constraints.remainingPaintExtent;
      for (var child = firstChild; child != null; child = childAfter(child)) {
        final offset = childScrollOffset(child);
        if (offset == null || !child.hasSize) continue;
        if (anchor == null &&
            offset < end &&
            offset + child.size.height > start) {
          anchor = child;
          before = offset + (reverse ? child.size.height : 0);
        }
        if (offset >= start && offset < end) {
          if (!reverse || offset + child.size.height <= end) {
            anchor = child;
            before = offset + (reverse ? child.size.height : 0);
            if (!reverse) break;
          }
        }
      }
    }
    super.performLayout();
    if (anchor == null ||
        anchor.parent != this ||
        geometry?.scrollOffsetCorrection != null) {
      return;
    }
    final offset = childScrollOffset(anchor);
    if (offset == null) return;
    final delta = offset + (reverse ? anchor.size.height : 0) - before!;
    if (delta.abs() > 0.01) {
      geometry = SliverGeometry(scrollOffsetCorrection: delta);
    }
  }
}

// Apply following during layout, before painting, including resized markdown
// and keyboard changes. User input disables it before the next layout pass.
class _TranscriptPhysics extends ScrollPhysics {
  const _TranscriptPhysics({required this.follow, super.parent});
  final bool Function() follow;

  @override
  _TranscriptPhysics applyTo(ScrollPhysics? ancestor) =>
      _TranscriptPhysics(follow: follow, parent: buildParent(ancestor));

  @override
  double adjustPositionForNewDimensions({
    required ScrollMetrics oldPosition,
    required ScrollMetrics newPosition,
    required bool isScrolling,
    required double velocity,
  }) => follow()
      ? newPosition.maxScrollExtent
      : super.adjustPositionForNewDimensions(
          oldPosition: oldPosition,
          newPosition: newPosition,
          isScrolling: isScrolling,
          velocity: velocity,
        );
}

/// Shown while one or more bots are composing a reply.
class _Typing extends StatelessWidget {
  const _Typing({required this.handles, this.status});

  final List<String> handles;
  final String? status;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final who = handles.length == 1
        ? '@${handles.first} ${status ?? 'is thinking'}'
        : '${handles.length} bots are thinking';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          BotAvatar(handle: handles.first, radius: 15),
          const SizedBox(width: 11),
          SizedBox(
            width: 13,
            height: 13,
            child: CircularProgressIndicator(
              strokeWidth: 2,
              color: scheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(width: 9),
          Expanded(
            child: Text(
              who,
              style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        ],
      ),
    );
  }
}

class _Bubble extends StatelessWidget {
  const _Bubble({
    super.key,
    required this.message,
    required this.store,
    this.desktop = false,
    this.pending = false,
    this.attachmentRoot,
    this.onOpenAttachment,
    this.onReply,
  });

  final ChatMessage message;
  final RoomStore store;
  final bool desktop;

  /// Sent, but the room has not confirmed it yet.
  final bool pending;
  final String? attachmentRoot;
  final void Function(ChatFile)? onOpenAttachment;
  final ValueChanged<ChatMessage>? onReply;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final author = message.author;

    if (author.isSystem) {
      if (desktop) {
        return _SystemNotice(message: message);
      }
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 7),
        child: Center(
          child: Text(
            message.body,
            textAlign: TextAlign.center,
            style: TextStyle(
              fontSize: 11,
              fontStyle: FontStyle.italic,
              color: scheme.onSurfaceVariant,
            ),
          ),
        ),
      );
    }

    final isHuman = author.isHuman;
    final name = isHuman
        ? (author.name ?? 'you')
        : store.handleFor(author.agentId);
    final displayName = isHuman
        ? name
        : store.agents
              .where((a) => a.id == author.agentId)
              .firstOrNull
              ?.displayName;
    final parent = message.replyTo == null
        ? null
        : store.messages.where((m) => m.id == message.replyTo).firstOrNull;
    final color = isHuman
        ? scheme.primary
        : handleColor(
            name,
            dark: Theme.of(context).brightness == Brightness.dark,
          );

    final time = message.sentAt.dateTime.toLocal();
    final content = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            if (isHuman)
              Icon(Icons.person_outline, size: desktop ? 17 : 19, color: color)
            else
              BotAvatar(handle: name, radius: desktop ? 10 : 12),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                displayName == null || displayName.isEmpty ? name : displayName,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontWeight: FontWeight.w600,
                  fontSize: 13,
                  color: color,
                ),
              ),
            ),
            const SizedBox(width: 8),
            if (pending)
              Text(
                'Pending',
                style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
              )
            else if (message.sentAt.millis > 0)
              Text(
                '${time.hour.toString().padLeft(2, '0')}:${time.minute.toString().padLeft(2, '0')}',
                style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
              ),
            if (!pending && onReply != null)
              IconButton(
                tooltip: 'Reply to $name',
                visualDensity: VisualDensity.compact,
                icon: const Icon(Icons.reply, size: 18),
                onPressed: () => onReply!(message),
              ),
          ],
        ),
        if (message.replyTo != null)
          Container(
            margin: const EdgeInsets.only(top: 8),
            padding: const EdgeInsets.fromLTRB(10, 4, 6, 4),
            decoration: BoxDecoration(
              border: Border(left: BorderSide(color: scheme.primary, width: 2)),
            ),
            child: Text(
              parent == null
                  ? 'Reply to earlier message'
                  : '${parent.author.isHuman ? parent.author.name ?? 'You' : store.handleFor(parent.author.agentId)}: ${parent.body}',
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: scheme.onSurfaceVariant, fontSize: 12),
            ),
          ),
        SizedBox(height: desktop ? 7 : 10),
        Opacity(
          opacity: pending ? 0.7 : 1,
          child: MessageText(text: message.body),
        ),
        for (final file in message.files)
          AttachmentTile(
            file: file,
            root: attachmentRoot,
            onOpen: onOpenAttachment == null
                ? null
                : () => onOpenAttachment!(file),
          ),
      ],
    );
    if (desktop) {
      final row = Padding(
        padding: const EdgeInsets.fromLTRB(12, 3, 10, 6),
        child: content,
      );
      return Padding(
        padding: const EdgeInsets.only(bottom: 12),
        child: isHuman
            ? DecoratedBox(
                decoration: BoxDecoration(
                  color: scheme.surfaceContainerLow.withValues(alpha: 0.45),
                  borderRadius: BorderRadius.circular(7),
                ),
                child: row,
              )
            : row,
      );
    }
    return Container(
      margin: const EdgeInsets.only(bottom: 20),
      padding: isHuman
          ? const EdgeInsets.all(14)
          : const EdgeInsets.symmetric(vertical: 12),
      decoration: BoxDecoration(
        color: isHuman ? scheme.surfaceContainerLow : null,
        borderRadius: BorderRadius.circular(16),
        border: isHuman
            ? Border.all(color: scheme.outlineVariant.withValues(alpha: 0.6))
            : null,
      ),
      child: content,
    );
  }
}

class _SystemNotice extends StatefulWidget {
  const _SystemNotice({required this.message});

  final ChatMessage message;

  @override
  State<_SystemNotice> createState() => _SystemNoticeState();
}

class _SystemNoticeState extends State<_SystemNotice> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Column(
        children: [
          Material(
            color: Colors.transparent,
            child: InkWell(
              borderRadius: BorderRadius.circular(6),
              onTap: () => setState(() => _expanded = !_expanded),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
                child: Row(
                  children: [
                    Icon(
                      Icons.info_outline,
                      size: 17,
                      color: scheme.onSurfaceVariant,
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(
                        widget.message.body,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 12,
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                    ),
                    Icon(
                      _expanded ? Icons.expand_less : Icons.expand_more,
                      size: 18,
                      color: scheme.onSurfaceVariant,
                    ),
                  ],
                ),
              ),
            ),
          ),
          if (_expanded)
            Padding(
              padding: const EdgeInsets.fromLTRB(35, 0, 12, 12),
              child: Align(
                alignment: Alignment.centerLeft,
                child: SelectableText(widget.message.body),
              ),
            ),
        ],
      ),
    );
  }
}

/// Shared rendering for messages and saved reports. Images never load implicitly.
class MessageText extends StatelessWidget {
  final String text;
  const MessageText({super.key, required this.text});
  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return MarkdownBody(
      data: text,
      softLineBreak: true,
      imageBuilder: (uri, title, alt) =>
          Text(alt?.isNotEmpty == true ? alt! : 'Image attachment'),
      onTapLink: (_, href, _) async {
        final uri = href == null ? null : Uri.tryParse(href);
        if (uri == null || !['https', 'http'].contains(uri.scheme)) return;
        try {
          await launchUrl(uri, mode: LaunchMode.externalApplication);
        } catch (_) {
          if (context.mounted) {
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(content: Text('Could not open this link.')),
            );
          }
        }
      },
      styleSheet: MarkdownStyleSheet.fromTheme(Theme.of(context)).copyWith(
        p: TextStyle(fontSize: 15, height: 1.55, color: scheme.onSurface),
        h1: TextStyle(
          fontSize: 21,
          height: 1.4,
          fontWeight: FontWeight.w600,
          color: scheme.onSurface,
        ),
        h2: TextStyle(
          fontSize: 18,
          height: 1.4,
          fontWeight: FontWeight.w600,
          color: scheme.onSurface,
        ),
        h3: TextStyle(
          fontSize: 16,
          height: 1.4,
          fontWeight: FontWeight.w600,
          color: scheme.onSurface,
        ),
        code: TextStyle(
          fontFamily: 'monospace',
          fontSize: 13,
          color: scheme.onSurface,
        ),
        codeblockDecoration: BoxDecoration(
          color: scheme.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(8),
        ),
        blockSpacing: 12,
      ),
    );
  }
}

/// A compact board across the top of the chat.
class BoardStrip extends StatelessWidget {
  const BoardStrip({
    super.key,
    required this.store,
    this.canAnswer = true,
    this.embedded = false,
  });

  final bool embedded;

  final RoomStore store;
  final bool canAnswer;

  Future<void> _answer(BuildContext context, Task task) async {
    var reply = '';
    final answer = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Reply to the team'),
        content: TextField(
          autofocus: true,
          minLines: 2,
          maxLines: 5,
          decoration: const InputDecoration(
            hintText: 'What has changed, or how should they proceed?',
          ),
          onChanged: (value) => reply = value,
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () {
              if (reply.trim().isNotEmpty) Navigator.pop(context, reply.trim());
            },
            child: const Text('Send reply'),
          ),
        ],
      ),
    );
    if (answer != null) store.answer(task.id, answer);
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final tasks = store.tasks;
    return Container(
      constraints: embedded ? null : const BoxConstraints(maxHeight: 210),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
        border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
      ),
      child: tasks.isEmpty
          ? Padding(
              padding: const EdgeInsets.all(16),
              child: Text(
                'The board is empty.',
                style: TextStyle(color: scheme.onSurfaceVariant),
              ),
            )
          : ListView(
              physics: embedded ? const NeverScrollableScrollPhysics() : null,
              shrinkWrap: true,
              padding: const EdgeInsets.all(12),
              children: [
                Padding(
                  padding: const EdgeInsets.only(bottom: 18, top: 4),
                  child: Text(
                    'Tasks · ${tasks.length}',
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
                for (final task in tasks)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 8),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        StatusPill(
                          color: taskColor(task.state),
                          label: task.state.label,
                        ),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(
                                task.title,
                                style: const TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                              if (task.assignee != null)
                                Text(
                                  '@' + store.handleFor(task.assignee),
                                  style: TextStyle(
                                    fontSize: 11,
                                    color: scheme.onSurfaceVariant,
                                  ),
                                ),
                              for (final reason in task.blockReasons)
                                Padding(
                                  padding: const EdgeInsets.only(top: 6),
                                  child: Text(
                                    (reason['prompt'] ??
                                            reason['description'] ??
                                            reason['detail'] ??
                                            reason['kind'] ??
                                            'Waiting for input')
                                        .toString(),
                                    style: TextStyle(
                                      fontSize: 12,
                                      color: scheme.onSurfaceVariant,
                                    ),
                                  ),
                                ),
                              if (task.blockReasons.any(
                                (r) => r['kind'] == 'awaiting_human',
                              ))
                                TextButton(
                                  onPressed: canAnswer
                                      ? () => _answer(context, task)
                                      : null,
                                  child: Text(
                                    canAnswer
                                        ? 'Reply to unblock'
                                        : 'Open the room to reply',
                                  ),
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
  const Composer({
    super.key,
    required this.controller,
    required this.onSend,
    this.mentions = const [],
    this.attachments = const [],
    this.attachmentRoot,
    this.onAttach,
    this.onRemoveAttachment,
    this.onOpenAttachment,
    this.busy = false,
    this.busyLabel = 'Preparing attachments…',
    this.focusNode,
    this.desktop = false,
    this.replyTo,
    this.replyAuthor,
    this.onCancelReply,
  });

  final TextEditingController controller;
  final VoidCallback onSend;
  final List<String> mentions;
  final List<ChatFile> attachments;
  final String? attachmentRoot;
  final VoidCallback? onAttach;
  final void Function(ChatFile)? onRemoveAttachment, onOpenAttachment;
  final bool busy;
  final String busyLabel;
  final FocusNode? focusNode;
  final bool desktop;
  final ChatMessage? replyTo;
  final String? replyAuthor;
  final VoidCallback? onCancelReply;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final composer = Container(
      margin: EdgeInsets.fromLTRB(desktop ? 24 : 16, 8, desktop ? 24 : 16, 12),
      padding: EdgeInsets.all(desktop ? 4 : 6),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(desktop ? 10 : 18),
        border: Border.all(color: scheme.outlineVariant.withValues(alpha: 0.8)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (replyTo != null)
            Padding(
              padding: const EdgeInsets.only(left: 12, top: 4),
              child: Row(
                children: [
                  Icon(Icons.reply, size: 18, color: scheme.primary),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'Reply to ${replyAuthor ?? 'message'}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: scheme.primary,
                            fontWeight: FontWeight.w600,
                            fontSize: 12,
                          ),
                        ),
                        Text(
                          replyTo!.body,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: scheme.onSurfaceVariant,
                            fontSize: 12,
                          ),
                        ),
                      ],
                    ),
                  ),
                  IconButton(
                    tooltip: 'Cancel reply',
                    onPressed: onCancelReply,
                    icon: const Icon(Icons.close, size: 18),
                  ),
                ],
              ),
            ),
          if (attachments.isNotEmpty)
            SizedBox(
              height: MediaQuery.textScalerOf(context).scale(76) + 36,
              child: ListView(
                scrollDirection: Axis.horizontal,
                children: [
                  for (final file in attachments)
                    AttachmentTile(
                      file: file,
                      root: attachmentRoot,
                      onOpen: onOpenAttachment == null
                          ? null
                          : () => onOpenAttachment!(file),
                      onRemove: busy || onRemoveAttachment == null
                          ? null
                          : () => onRemoveAttachment!(file),
                    ),
                ],
              ),
            ),
          if (busy)
            Padding(
              padding: EdgeInsets.all(8),
              child: Row(
                children: [
                  SizedBox(
                    width: 16,
                    height: 16,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                  SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      busyLabel,
                      style: const TextStyle(fontSize: 12),
                    ),
                  ),
                ],
              ),
            ),
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              if (onAttach != null || mentions.isNotEmpty)
                PopupMenuButton<String>(
                  tooltip: desktop ? 'Add files or mention' : 'Add to message',
                  enabled: !busy,
                  icon: const Icon(Icons.add_rounded, size: 24),
                  itemBuilder: (_) => [
                    if (onAttach != null)
                      const PopupMenuItem(
                        value: 'file',
                        child: Row(
                          children: [
                            Icon(Icons.attach_file, size: 18),
                            SizedBox(width: 12),
                            Text('Attach files'),
                          ],
                        ),
                      ),
                    if (onAttach != null && mentions.isNotEmpty)
                      const PopupMenuDivider(),
                    if (mentions.isNotEmpty)
                      const PopupMenuItem(value: '@all', child: Text('@all')),
                    for (final handle in mentions)
                      PopupMenuItem(value: '@$handle', child: Text('@$handle')),
                  ],
                  onSelected: (value) {
                    if (value == 'file') {
                      onAttach?.call();
                      return;
                    }
                    final selection = controller.selection;
                    final start = selection.isValid
                        ? selection.start
                        : controller.text.length;
                    final end = selection.isValid ? selection.end : start;
                    final tag =
                        '${start > 0 && !RegExp(r'\s').hasMatch(controller.text[start - 1]) ? ' ' : ''}$value ';
                    controller.value = TextEditingValue(
                      text: controller.text.replaceRange(start, end, tag),
                      selection: TextSelection.collapsed(
                        offset: start + tag.length,
                      ),
                    );
                  },
                ),
              Expanded(
                child: CallbackShortcuts(
                  bindings: {
                    if (replyTo != null)
                      const SingleActivator(LogicalKeyboardKey.escape): () =>
                          onCancelReply?.call(),
                    const SingleActivator(
                      LogicalKeyboardKey.enter,
                      control: true,
                    ): () {
                      if (!busy) onSend();
                    },
                  },
                  child: TextField(
                    controller: controller,
                    focusNode: focusNode,
                    enabled: !busy,
                    minLines: 1,
                    maxLines: 5,
                    style: const TextStyle(fontSize: 14, height: 1.5),
                    textInputAction: TextInputAction.send,
                    onSubmitted: (_) {
                      if (!busy &&
                          (controller.text.trim().isNotEmpty ||
                              attachments.isNotEmpty)) {
                        onSend();
                      }
                    },
                    decoration: InputDecoration(
                      hintText: 'Message or @mention a teammate…',
                      filled: false,
                      border: InputBorder.none,
                      enabledBorder: InputBorder.none,
                      focusedBorder: InputBorder.none,
                      disabledBorder: InputBorder.none,
                      contentPadding: EdgeInsets.symmetric(
                        horizontal: 6,
                        vertical: desktop ? 8 : 12,
                      ),
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 6),
              ValueListenableBuilder<TextEditingValue>(
                valueListenable: controller,
                builder: (context, value, _) => Tooltip(
                  message: desktop
                      ? 'Send message · Ctrl+Enter'
                      : 'Send message',
                  child: FilledButton(
                    onPressed:
                        busy ||
                            (value.text.trim().isEmpty && attachments.isEmpty)
                        ? null
                        : onSend,
                    style: FilledButton.styleFrom(
                      minimumSize: Size.square(desktop ? 40 : 44),
                      shape: desktop
                          ? RoundedRectangleBorder(
                              borderRadius: BorderRadius.circular(8),
                            )
                          : const CircleBorder(),
                      padding: EdgeInsets.all(desktop ? 10 : 12),
                    ),
                    child: Icon(
                      desktop ? Icons.send_rounded : Icons.arrow_upward_rounded,
                      size: desktop ? 18 : 20,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ],
      ),
    );
    return desktop
        ? Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 968),
              child: composer,
            ),
          )
        : composer;
  }
}

class AttachmentTile extends StatelessWidget {
  const AttachmentTile({
    super.key,
    required this.file,
    this.root,
    this.onOpen,
    this.onRemove,
  });
  final ChatFile file;
  final String? root;
  final VoidCallback? onOpen, onRemove;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final preview =
        root != null &&
        file.images > 0 &&
        RegExp(r'^att_[a-f0-9]{32}$').hasMatch(file.id);
    final size = file.bytes < 1024 * 1024
        ? '${(file.bytes / 1024).ceil()} KB'
        : '${(file.bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
    return Container(
      width: 264,
      margin: const EdgeInsets.only(right: 8, top: 6, bottom: 6),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHigh,
        border: Border.all(color: scheme.outlineVariant),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Tooltip(
        message: '${file.name}\n${file.note}',
        child: InkWell(
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Row(
              children: [
                if (preview)
                  ClipRRect(
                    borderRadius: BorderRadius.circular(6),
                    child: Image.file(
                      File('$root/${file.id}/image-0.jpg'),
                      width: 48,
                      height: 64,
                      fit: BoxFit.cover,
                      cacheWidth: 128,
                      errorBuilder: (_, _, _) =>
                          const Icon(Icons.image_outlined, size: 32),
                    ),
                  )
                else
                  Icon(
                    file.images > 0
                        ? Icons.image_outlined
                        : Icons.description_outlined,
                    size: 32,
                    color: scheme.primary,
                  ),
                const SizedBox(width: 8),
                Expanded(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        file.name,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      Text(
                        size,
                        style: TextStyle(
                          fontSize: 11,
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                      if (file.note.isNotEmpty)
                        Text(
                          file.note,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 10,
                            color: scheme.onSurfaceVariant,
                          ),
                        ),
                    ],
                  ),
                ),
                if (onRemove != null)
                  IconButton(
                    tooltip: 'Remove ${file.name}',
                    onPressed: onRemove,
                    icon: const Icon(Icons.close, size: 18),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Shows an unsuccessful turn without presenting an active retry as failed.
///
/// Every bot failing to take a turn used to be indistinguishable from every
/// bot having nothing to do. The most common cause is a backend that will not
/// start — a project folder that does not exist, or a CLI that is not signed
/// in — and the message from the provider usually names it exactly.
class _FailureBanner extends StatelessWidget {
  const _FailureBanner({required this.failing});

  final List<BotActivity> failing;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final worst = failing.first;
    return Material(
      color: scheme.surfaceContainerLow,
      child: InkWell(
        onTap: () => showModalBottomSheet<void>(
          context: context,
          showDragHandle: true,
          isScrollControlled: true,
          builder: (_) => SafeArea(
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  for (final entry in failing) ...[
                    Text(
                      entry.agent.displayName.isEmpty
                          ? '@${entry.agent.handle}'
                          : entry.agent.displayName,
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 8),
                    SelectableText(
                      '${entry.lastError ?? 'The backend did not respond'}${_BotRow._ago(entry.lastErrorAt)}',
                    ),
                    const SizedBox(height: 20),
                  ],
                ],
              ),
            ),
          ),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
          child: Row(
            children: [
              Icon(Icons.error_outline, size: 17, color: scheme.error),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  failing.length == 1
                      ? '@${worst.agent.handle} — previous turn failed'
                      : '${failing.length} bots have previous turn failures',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ),
              const Icon(Icons.chevron_right, size: 18),
            ],
          ),
        ),
      ),
    );
  }
}
