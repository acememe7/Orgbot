// The pieces a room is made of: the activity rail, the transcript, the board
// strip and the composer.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart' show PointerScrollEvent;
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'dart:io';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:url_launcher/url_launcher.dart';

import '../orgbot/room_check.dart' show expandHome;
import '../protocol/models.dart';
import '../state/room_store.dart';
import '../workspace/desktop_files.dart';
import 'brand.dart';
import 'polish.dart';
import 'widgets/mentions.dart';

export 'widgets/mentions.dart' show MentionOption;

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
            style: TextStyle(fontWeight: FontWeight.w600, fontSize: 13.5),
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
            child: TweenAnimationBuilder<double>(
              tween: Tween(end: tasks.isEmpty ? 0 : done / tasks.length),
              duration: motionDuration(context, 320),
              curve: Curves.easeOutCubic,
              builder: (context, value, _) => LinearProgressIndicator(
                value: value,
                minHeight: 4,
                backgroundColor: scheme.surfaceContainerHighest,
              ),
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

    final live = entry.streaming;
    return AnimatedContainer(
      duration: motionDuration(context, 180),
      curve: Curves.easeOut,
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
      decoration: BoxDecoration(
        color: live
            ? scheme.primary.withValues(alpha: 0.05)
            : scheme.surfaceContainerLow.withValues(alpha: 0.6),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(
          color: live
              ? scheme.primary.withValues(alpha: 0.35)
              : scheme.outlineVariant.withValues(alpha: 0.7),
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
                    Text(
                      entry.agent.displayName.isEmpty
                          ? '@${entry.agent.handle}'
                          : entry.agent.displayName,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontWeight: FontWeight.w600,
                        fontSize: 13,
                      ),
                    ),
                    if (entry.agent.roles.isNotEmpty)
                      Text(
                        entry.agent.roles
                            .map((r) => r.replaceAll('_', ' '))
                            .join(', '),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 11,
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              Flexible(
                child: AnimatedSwitcher(
                  duration: motionDuration(context, 160),
                  switchInCurve: Curves.easeOut,
                  switchOutCurve: Curves.easeIn,
                  layoutBuilder: (current, previous) => Stack(
                    alignment: Alignment.centerRight,
                    children: [...previous, ?current],
                  ),
                  child: StatusPill(
                    key: ValueKey(label),
                    color: color,
                    label: label,
                  ),
                ),
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

  /// Messages present when the transcript opened never animate in; only
  /// arrivals do, and each one only once.
  Set<String>? _initialIds;
  final Set<String> _arrived = {};

  /// The failures the banner was closed for. A new failure shows it again.
  String? _dismissedFailures;

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
    _initialIds ??= {for (final m in messages) m.id};
    final boundary = messages.indexWhere((m) => m.id == _boundary);
    final split = boundary + 1;
    final composing = store.activity.where((a) => a.streaming).toList();
    final failing = store.failing;
    final failureKey = [
      for (final f in failing) '${f.agent.id}@${f.lastErrorAt}',
    ].join(',');
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
            message: message,
            store: store,
            desktop: widget.desktop,
            pending: store.isPending(message),
            attachmentRoot: widget.attachmentRoot,
            onOpenAttachment: widget.onOpenAttachment,
            onReply: widget.onReply,
          );
          final animate =
              !_initialIds!.contains(message.id) && _arrived.add(message.id);
          final arriving = _Arrival(animate: animate, child: bubble);
          return widget.desktop
              ? Align(
                  key: ValueKey(message.id),
                  alignment: Alignment.center,
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 880),
                    child: arriving,
                  ),
                )
              : KeyedSubtree(key: ValueKey(message.id), child: arriving);
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
            onPointerSignal: (signal) {
              // A wheel turned down at the bottom moves nothing and sends no
              // scroll notifications, so it must not leave the pill showing.
              if (signal is PointerScrollEvent && signal.scrollDelta.dy > 0) {
                WidgetsBinding.instance.addPostFrameCallback((_) {
                  if (!mounted || !widget.controller.hasClients) return;
                  final atEnd = widget.controller.position.extentAfter <= 24;
                  if (atEnd != _following) setState(() => _following = atEnd);
                });
                return;
              }
              _hold();
            },
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
        if (messages.isEmpty && composing.isEmpty && failing.isEmpty)
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
        if (failing.isNotEmpty && failureKey != _dismissedFailures)
          Positioned(
            top: 8,
            left: 16,
            right: 16,
            child: Center(
              child: _PopIn(
                child: _FailureBanner(
                  failing: failing,
                  onDismiss: () =>
                      setState(() => _dismissedFailures = failureKey),
                ),
              ),
            ),
          ),
        if (composing.isNotEmpty && _following)
          Positioned(
            left: 20,
            right: 20,
            bottom: 0,
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: scheme.surface,
                borderRadius: BorderRadius.circular(8),
              ),
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
              child: _PopIn(
                child: FilledButton.tonalIcon(
                  onPressed: _latest,
                  style: FilledButton.styleFrom(
                    minimumSize: const Size(0, 32),
                    padding: const EdgeInsets.symmetric(horizontal: 14),
                    shape: const StadiumBorder(),
                    textStyle: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  icon: const Icon(Icons.arrow_downward, size: 15),
                  label: const Text('Jump to latest'),
                ),
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
        : handles.length <= 3
        ? '${handles.map((h) => '@$h').join(', ')} are thinking'
        : '${handles.length} bots are thinking';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          SizedBox(
            width: 30.0 + 18.0 * (handles.length.clamp(1, 3) - 1),
            height: 30,
            child: Stack(
              children: [
                for (var i = 0; i < handles.length.clamp(1, 3); i++)
                  Positioned(
                    left: 18.0 * i,
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        border: Border.all(color: scheme.surface, width: 1.5),
                      ),
                      child: BotAvatar(handle: handles[i], radius: 13.5),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          const _Dots(),
          const SizedBox(width: 8),
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

/// Three dots that breathe in turn. Still when motion is reduced.
class _Dots extends StatefulWidget {
  const _Dots();

  @override
  State<_Dots> createState() => _DotsState();
}

class _DotsState extends State<_Dots> with SingleTickerProviderStateMixin {
  late final AnimationController _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1100),
  );

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _pulse.stop();
    } else if (!_pulse.isAnimating) {
      _pulse.repeat();
    }
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurfaceVariant;
    return RepaintBoundary(
      child: AnimatedBuilder(
        animation: _pulse,
        builder: (context, _) => Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (var i = 0; i < 3; i++) ...[
              if (i > 0) const SizedBox(width: 3),
              Container(
                width: 5,
                height: 5,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: color.withValues(alpha: _alpha(i)),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }

  double _alpha(int i) {
    if (!_pulse.isAnimating) return 0.7;
    final t = (_pulse.value - i * 0.18) % 1.0;
    // A short rise and a slower fall, so the motion reads as calm.
    final wave = t < 0.3 ? t / 0.3 : (t < 0.7 ? 1 - (t - 0.3) / 0.4 : 0.0);
    return 0.3 + 0.6 * wave;
  }
}

/// Fades and lifts a newly arrived message into place once.
///
/// Only paint is animated: the row is laid out at its final size from the
/// first frame, so scroll anchoring and following are unaffected.
class _Arrival extends StatefulWidget {
  const _Arrival({required this.animate, required this.child});

  final bool animate;
  final Widget child;

  @override
  State<_Arrival> createState() => _ArrivalState();
}

class _ArrivalState extends State<_Arrival>
    with SingleTickerProviderStateMixin {
  AnimationController? _controller;

  @override
  void initState() {
    super.initState();
    if (widget.animate) {
      _controller = AnimationController(
        vsync: this,
        duration: const Duration(milliseconds: 220),
      );
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final controller = _controller;
    if (controller == null || controller.isAnimating || controller.isCompleted) {
      return;
    }
    if (MediaQuery.disableAnimationsOf(context)) {
      controller.value = 1;
    } else {
      controller.forward();
    }
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    if (controller == null) return widget.child;
    final curve = CurvedAnimation(
      parent: controller,
      curve: Curves.easeOutCubic,
    );
    return AnimatedBuilder(
      animation: curve,
      child: widget.child,
      builder: (context, child) => controller.isCompleted
          ? child!
          : Opacity(
              opacity: curve.value,
              child: Transform.translate(
                offset: Offset(0, 6 * (1 - curve.value)),
                child: child,
              ),
            ),
    );
  }
}

/// A small entrance for floating controls: fade and a slight scale.
class _PopIn extends StatefulWidget {
  const _PopIn({required this.child});

  final Widget child;

  @override
  State<_PopIn> createState() => _PopInState();
}

class _PopInState extends State<_PopIn> with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 160),
  );
  late final Animation<double> _curve = CurvedAnimation(
    parent: _controller,
    curve: Curves.easeOutCubic,
  );

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_controller.isCompleted || _controller.isAnimating) return;
    if (MediaQuery.disableAnimationsOf(context)) {
      _controller.value = 1;
    } else {
      _controller.forward();
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => FadeTransition(
    opacity: _curve,
    child: ScaleTransition(
      scale: Tween(begin: 0.96, end: 1.0).animate(_curve),
      child: widget.child,
    ),
  );
}

/// The message a reply answers, shown compactly above the reply.
class _ReplyQuote extends StatelessWidget {
  const _ReplyQuote({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(8, 4, 10, 4),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.subdirectory_arrow_right_rounded,
              size: 14, color: scheme.onSurfaceVariant),
          const SizedBox(width: 6),
          Flexible(
            child: Text(
              label,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(color: scheme.onSurfaceVariant, fontSize: 12),
            ),
          ),
        ],
      ),
    );
  }
}

/// Tracks pointer hover and keyboard focus within a row, for desktop actions
/// that stay out of the way until you reach for them.
class _HoverRow extends StatefulWidget {
  const _HoverRow({required this.builder});

  final Widget Function(BuildContext context, bool active) builder;

  @override
  State<_HoverRow> createState() => _HoverRowState();
}

class _HoverRowState extends State<_HoverRow> {
  bool _hovered = false;
  bool _focused = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
    onEnter: (_) => setState(() => _hovered = true),
    onExit: (_) => setState(() => _hovered = false),
    child: Focus(
      canRequestFocus: false,
      skipTraversal: true,
      onFocusChange: (focused) => setState(() => _focused = focused),
      child: widget.builder(context, _hovered || _focused),
    ),
  );
}

class _Bubble extends StatelessWidget {
  const _Bubble({
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
        : store.agentById(author.agentId)?.displayName;
    final shownName = displayName == null || displayName.isEmpty
        ? name
        : displayName;
    final parent = message.replyTo == null
        ? null
        : store.messageById(message.replyTo!);
    final color = isHuman
        ? scheme.primary
        : handleColor(
            name,
            dark: Theme.of(context).brightness == Brightness.dark,
          );
    final quote = message.replyTo == null
        ? null
        : _ReplyQuote(
            label: parent == null
                ? 'Reply to earlier message'
                : '${parent.author.isHuman ? parent.author.name ?? 'You' : store.handleFor(parent.author.agentId)}: ${parent.body}',
          );
    final time = message.sentAt.dateTime.toLocal();
    final stamp = pending
        ? 'Pending'
        : message.sentAt.millis > 0
        ? '${time.hour.toString().padLeft(2, '0')}:${time.minute.toString().padLeft(2, '0')}'
        : null;
    final body = Opacity(
      opacity: pending ? 0.6 : 1,
      child: MessageText(text: message.body),
    );
    final files = [
      for (final file in message.files)
        AttachmentTile(
          file: file,
          root: attachmentRoot,
          onOpen: onOpenAttachment == null
              ? null
              : () => onOpenAttachment!(file),
        ),
    ];
    Widget reply({double size = 18}) => IconButton(
      tooltip: 'Reply to $name',
      visualDensity: VisualDensity.compact,
      icon: Icon(Icons.reply, size: size),
      onPressed: () => onReply!(message),
    );

    if (desktop) {
      final avatar = isHuman
          ? CircleAvatar(
              radius: 14,
              backgroundColor: color.withValues(alpha: 0.16),
              child: Icon(Icons.person_outline, size: 16, color: color),
            )
          : BotAvatar(handle: name, radius: 14);
      return _HoverRow(
        builder: (context, active) => Container(
          margin: const EdgeInsets.only(bottom: 2),
          padding: const EdgeInsets.fromLTRB(10, 8, 6, 10),
          decoration: BoxDecoration(
            color: active
                ? scheme.onSurface.withValues(alpha: 0.035)
                : isHuman
                ? scheme.primary.withValues(alpha: 0.045)
                : null,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(padding: const EdgeInsets.only(top: 1), child: avatar),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(
                      height: 28,
                      child: Row(
                        children: [
                          Flexible(
                            child: Text(
                              shownName,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontWeight: FontWeight.w600,
                                fontSize: 13.5,
                                color: scheme.onSurface,
                              ),
                            ),
                          ),
                          if (!isHuman && shownName != name) ...[
                            const SizedBox(width: 6),
                            Text(
                              '@$name',
                              style: TextStyle(
                                fontSize: 12,
                                color: color,
                              ),
                            ),
                          ],
                          if (stamp != null) ...[
                            const SizedBox(width: 8),
                            Text(
                              stamp,
                              style: TextStyle(
                                fontSize: 11.5,
                                color: scheme.onSurfaceVariant,
                              ),
                            ),
                          ],
                          const Spacer(),
                          if (!pending && onReply != null)
                            AnimatedOpacity(
                              opacity: active ? 1 : 0,
                              duration: motionDuration(context, 120),
                              child: reply(size: 17),
                            ),
                        ],
                      ),
                    ),
                    ?quote,
                    const SizedBox(height: 2),
                    body,
                    ...files,
                  ],
                ),
              ),
            ],
          ),
        ),
      );
    }

    final content = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            if (isHuman)
              Icon(Icons.person_outline, size: 19, color: color)
            else
              BotAvatar(handle: name, radius: 12),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                shownName,
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
            if (stamp != null)
              Text(
                stamp,
                style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
              ),
            if (!pending && onReply != null) reply(),
          ],
        ),
        ?quote,
        const SizedBox(height: 10),
        body,
        ...files,
      ],
    );
    return Container(
      margin: const EdgeInsets.only(bottom: 20),
      padding: isHuman
          ? const EdgeInsets.all(14)
          : const EdgeInsets.symmetric(vertical: 12),
      decoration: BoxDecoration(
        color: isHuman ? scheme.surfaceContainerLow : null,
        borderRadius: BorderRadius.circular(14),
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

/// Lets links in messages open files in the room's project on this computer.
///
/// Bots link what they made (screenshots, patches, logs) by its path in the
/// project. Without this those links did nothing at all.
class RoomFileLinks extends InheritedWidget {
  const RoomFileLinks({
    super.key,
    required this.projectRoot,
    required this.open,
    required super.child,
  });

  /// The room's project folder, if it has one here.
  final String? projectRoot;

  /// Shows a project file, given its path.
  final ValueChanged<String>? open;

  static RoomFileLinks? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<RoomFileLinks>();

  /// `href` as a path inside the project, or null when it names anything
  /// else. Only a first filter: whatever reads the file checks again with
  /// symbolic links resolved.
  String? resolve(String? href) {
    final raw = projectRoot?.trim();
    if (href == null || href.isEmpty || raw == null || raw.isEmpty) return null;
    final root = Uri.directory(expandHome(raw)).normalizePath().toFilePath();
    final uri = Uri.tryParse(href);
    final String path;
    if (uri != null && uri.scheme == 'file') {
      path = uri.toFilePath();
    } else if (href.startsWith('/')) {
      path = Uri.decodeFull(href);
    } else if (uri == null || uri.hasScheme || href.startsWith('#')) {
      return null;
    } else {
      path = '$root${Uri.decodeFull(href)}';
    }
    final normal = Uri.file(path).normalizePath().toFilePath();
    return normal.startsWith(root) && normal.length > root.length ? normal : null;
  }

  @override
  bool updateShouldNotify(RoomFileLinks oldWidget) =>
      oldWidget.projectRoot != projectRoot || oldWidget.open != open;
}

const _previewableImages = {'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'};

/// Pixels a preview may decode. A small file can claim enormous dimensions
/// and take the app's memory once decoded; such an image is not previewed.
const _maxPreviewPixels = 40 * 1000 * 1000;

/// Image bytes, once their declared size is known to be safe to decode. Only
/// the header is read for the check.
Future<Uint8List> checkedImage(Future<Uint8List> bytes) async {
  final data = await bytes;
  final buffer = await ui.ImmutableBuffer.fromUint8List(data);
  final descriptor = await ui.ImageDescriptor.encoded(buffer);
  final pixels = descriptor.width * descriptor.height;
  descriptor.dispose();
  buffer.dispose();
  if (pixels > _maxPreviewPixels) {
    throw const FormatException('Too large to preview');
  }
  return data;
}

bool _isImagePath(String path) =>
    _previewableImages.contains(path.split('.').last.toLowerCase());

/// A project image shown inside a message, read through the same checked path
/// as the project pane.
class ProjectImageThumb extends StatefulWidget {
  const ProjectImageThumb({
    super.key,
    required this.root,
    required this.path,
    this.onTap,
  });

  final String root;
  final String path;
  final VoidCallback? onTap;

  @override
  State<ProjectImageThumb> createState() => _ProjectImageThumbState();
}

class _ProjectImageThumbState extends State<ProjectImageThumb> {
  // A message list rebuilds often; the same screenshot is read once.
  static final _cache = <String, Future<Uint8List>>{};
  late Future<Uint8List> _bytes;

  Future<Uint8List> _load() {
    if (_cache.length > 64) _cache.remove(_cache.keys.first);
    return _cache[widget.path] ??= checkedImage(
      DesktopFiles.readProjectFile(expandHome(widget.root), widget.path),
    ).catchError((Object error) {
      _cache.remove(widget.path);
      throw error;
    });
  }

  @override
  void initState() {
    super.initState();
    _bytes = _load();
  }

  @override
  void didUpdateWidget(covariant ProjectImageThumb oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.path != widget.path) _bytes = _load();
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final name = widget.path.split('/').last;
    return Tooltip(
      message: name,
      child: InkWell(
        onTap: widget.onTap,
        borderRadius: BorderRadius.circular(8),
        child: Container(
          width: 168,
          height: 112,
          clipBehavior: Clip.antiAlias,
          decoration: BoxDecoration(
            color: scheme.surfaceContainerHighest.withValues(alpha: 0.6),
            border: Border.all(color: scheme.outlineVariant),
            borderRadius: BorderRadius.circular(8),
          ),
          child: FutureBuilder<Uint8List>(
            future: _bytes,
            builder: (context, snapshot) {
              if (snapshot.hasData) {
                return Image.memory(
                  snapshot.data!,
                  fit: BoxFit.cover,
                  cacheWidth: 336,
                  semanticLabel: name,
                  errorBuilder: (_, _, _) => _unavailable(scheme, name),
                );
              }
              if (snapshot.hasError) return _unavailable(scheme, name);
              return const SizedBox.expand();
            },
          ),
        ),
      ),
    );
  }

  Widget _unavailable(ColorScheme scheme, String name) => Center(
    child: Padding(
      padding: const EdgeInsets.all(8),
      child: Text(
        '$name\nnot available',
        textAlign: TextAlign.center,
        style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
      ),
    ),
  );
}

/// Shared rendering for messages and saved reports. Remote images never load;
/// images in the room's own project show as thumbnails.
class MessageText extends StatelessWidget {
  final String text;
  const MessageText({super.key, required this.text});

  // One style sheet per theme rather than one per message per build.
  static final _sheets = Expando<MarkdownStyleSheet>();
  static final _mentions = Expando<Map<String, MarkdownElementBuilder>>();

  static MarkdownStyleSheet _sheetFor(ThemeData theme) =>
      _sheets[theme] ??= _buildSheet(theme);

  static MarkdownStyleSheet _buildSheet(ThemeData theme) {
    final scheme = theme.colorScheme;
    return MarkdownStyleSheet.fromTheme(theme).copyWith(
      p: TextStyle(fontSize: 14.5, height: 1.55, color: scheme.onSurface),
      h1: TextStyle(
        fontSize: 20,
        height: 1.4,
        fontWeight: FontWeight.w600,
        color: scheme.onSurface,
      ),
      h2: TextStyle(
        fontSize: 17,
        height: 1.4,
        fontWeight: FontWeight.w600,
        color: scheme.onSurface,
      ),
      h3: TextStyle(
        fontSize: 15.5,
        height: 1.4,
        fontWeight: FontWeight.w600,
        color: scheme.onSurface,
      ),
      a: TextStyle(
        color: scheme.primary,
        decoration: TextDecoration.underline,
        decorationColor: scheme.primary.withValues(alpha: 0.5),
      ),
      code: TextStyle(
        fontFamily: 'monospace',
        fontSize: 13,
        color: scheme.onSurface,
        backgroundColor: scheme.surfaceContainerHighest.withValues(alpha: 0.6),
      ),
      codeblockDecoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.6),
        borderRadius: BorderRadius.circular(6),
      ),
      codeblockPadding: const EdgeInsets.all(12),
      blockquoteDecoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(6),
      ),
      blockSpacing: 10,
    );
  }

  static Map<String, MarkdownElementBuilder> _buildersFor(ThemeData theme) =>
      _mentions[theme] ??= {
        'mention': MentionBuilder(
          TextStyle(
            color: theme.colorScheme.primary,
            fontWeight: FontWeight.w600,
          ),
        ),
      };

  // Plain links, not `![]()` images, which the image builder already shows.
  static final _linkTargets = RegExp(r'(?<!!)\[[^\]]*\]\(\s*<?([^)\s>]+)>?');

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final links = RoomFileLinks.maybeOf(context);
    final root = links?.projectRoot;
    Widget thumb(String path) => ProjectImageThumb(
      root: root!,
      path: path,
      onTap: links?.open == null ? null : () => links!.open!(path),
    );
    final images = links == null
        ? const <String>[]
        : {
            for (final match in _linkTargets.allMatches(text))
              if (links.resolve(match.group(1)) case final path?)
                if (_isImagePath(path)) path,
          }.take(8).toList();
    final body = MarkdownBody(
      data: text,
      softLineBreak: true,
      inlineSyntaxes: [MentionSyntax()],
      builders: _buildersFor(theme),
      imageBuilder: (uri, title, alt) {
        final path = links?.resolve(uri.toString());
        if (path != null && _isImagePath(path)) return thumb(path);
        return Text(alt?.isNotEmpty == true ? alt! : 'Image attachment');
      },
      onTapLink: (_, href, _) async {
        final path = links?.resolve(href);
        if (path != null && links?.open != null) {
          links!.open!(path);
          return;
        }
        final uri = href == null ? null : Uri.tryParse(href);
        if (uri == null || !['https', 'http'].contains(uri.scheme)) {
          if (href != null && (href.startsWith('/') || uri?.scheme == 'file')) {
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(
                content: Text(
                  'That file is outside this room\'s project, so it is not opened here.',
                ),
              ),
            );
          }
          return;
        }
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
      styleSheet: _sheetFor(theme),
    );
    if (images.isEmpty) return body;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        body,
        const SizedBox(height: 8),
        Wrap(spacing: 8, runSpacing: 8, children: [for (final path in images) thumb(path)]),
      ],
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

  Future<void> _park(BuildContext context, Task task) async {
    var reason = '';
    final park = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Park this task?'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Nobody will work on “${task.title}”, claim it or ask you about '
              'it until you resume it. Questions waiting on it are withdrawn.',
            ),
            const SizedBox(height: 12),
            TextField(
              autofocus: true,
              decoration: const InputDecoration(
                hintText: 'Why, for the team (optional)',
              ),
              onChanged: (value) => reason = value,
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Park task'),
          ),
        ],
      ),
    );
    if (park == true) store.parkTask(task.id, reason: reason.trim());
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
                              if (task.state == TaskState.parked && canAnswer)
                                TextButton(
                                  onPressed: () => store.resumeTask(task.id),
                                  child: const Text('Resume task'),
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
                        if (canAnswer && task.state.isParkable)
                          IconButton(
                            tooltip: 'Park until you resume it',
                            visualDensity: VisualDensity.compact,
                            iconSize: 18,
                            icon: const Icon(Icons.pause_circle_outline),
                            onPressed: () => _park(context, task),
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
    case TaskState.parked:
      return Colors.brown;
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
class Composer extends StatefulWidget {
  const Composer({
    super.key,
    required this.controller,
    required this.onSend,
    this.mentions = const [],
    this.mentionOptions,
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

  /// Bot handles that can be tagged. [mentionOptions] adds names and roles.
  final List<String> mentions;
  final List<MentionOption>? mentionOptions;
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
  State<Composer> createState() => _ComposerState();
}

class _ComposerState extends State<Composer> {
  FocusNode? _ownFocus;
  FocusNode get _focus => widget.focusNode ?? (_ownFocus ??= FocusNode());
  bool _focused = false;
  MentionQuery? _query;
  List<MentionOption> _suggestions = const [];
  int _highlighted = 0;

  /// The query the suggestions were dismissed for, so Esc sticks.
  int? _dismissedAt;

  List<MentionOption> get _options {
    final options =
        widget.mentionOptions ??
        [for (final handle in widget.mentions) MentionOption(handle: handle)];
    return [if (options.isNotEmpty) MentionOption.all, ...options];
  }

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_textChanged);
    _focus.addListener(_focusChanged);
    _focused = _focus.hasFocus;
    // A restored draft may already end in an `@`.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _textChanged();
    });
  }

  @override
  void didUpdateWidget(Composer old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller.removeListener(_textChanged);
      widget.controller.addListener(_textChanged);
    }
    if (old.focusNode != widget.focusNode) {
      (old.focusNode ?? _ownFocus)?.removeListener(_focusChanged);
      _focus.addListener(_focusChanged);
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_textChanged);
    _focus.removeListener(_focusChanged);
    _ownFocus?.dispose();
    super.dispose();
  }

  void _focusChanged() {
    if (_focused == _focus.hasFocus) return;
    setState(() => _focused = _focus.hasFocus);
  }

  void _textChanged() {
    final query = MentionQuery.at(widget.controller.value);
    if (query?.start != _dismissedAt) _dismissedAt = null;
    final suggestions = query == null || _dismissedAt != null
        ? const <MentionOption>[]
        : _options.where((o) => o.matches(query.text)).toList();
    if (query?.start == _query?.start &&
        query?.text == _query?.text &&
        suggestions.length == _suggestions.length) {
      return;
    }
    setState(() {
      _query = query;
      _suggestions = suggestions;
      _highlighted = 0;
    });
  }

  void _accept(MentionOption option) {
    final query = _query;
    if (query == null) return;
    widget.controller.value = query.complete(
      widget.controller.value,
      option.handle,
    );
    _focus.requestFocus();
  }

  void _move(int by) => setState(
    () => _highlighted = (_highlighted + by) % _suggestions.length,
  );

  void _insertTag(String value) {
    final controller = widget.controller;
    final selection = controller.selection;
    final start = selection.isValid ? selection.start : controller.text.length;
    final end = selection.isValid ? selection.end : start;
    final tag =
        '${start > 0 && !RegExp(r'\s').hasMatch(controller.text[start - 1]) ? ' ' : ''}$value ';
    controller.value = TextEditingValue(
      text: controller.text.replaceRange(start, end, tag),
      selection: TextSelection.collapsed(offset: start + tag.length),
    );
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final desktop = widget.desktop;
    final busy = widget.busy;
    final attachments = widget.attachments;
    final controller = widget.controller;
    final suggesting = _suggestions.isNotEmpty && _focused;
    final composer = AnimatedContainer(
      duration: motionDuration(context, 140),
      curve: Curves.easeOut,
      margin: EdgeInsets.fromLTRB(desktop ? 24 : 16, 8, desktop ? 24 : 16, 4),
      padding: EdgeInsets.all(desktop ? 4 : 6),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(desktop ? 10 : 18),
        border: Border.all(
          color: _focused
              ? scheme.primary.withValues(alpha: 0.55)
              : scheme.outlineVariant.withValues(alpha: 0.8),
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedSize(
            duration: motionDuration(context, 120),
            curve: Curves.easeOut,
            alignment: Alignment.bottomCenter,
            child: suggesting
                ? Padding(
                    padding: const EdgeInsets.fromLTRB(2, 2, 2, 4),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        MentionSuggestions(
                          options: _suggestions,
                          highlighted: _highlighted,
                          onSelected: _accept,
                          onHover: (i) => setState(() => _highlighted = i),
                        ),
                        const Divider(height: 1),
                      ],
                    ),
                  )
                : const SizedBox(width: double.infinity),
          ),
          if (widget.replyTo != null)
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
                          'Reply to ${widget.replyAuthor ?? 'message'}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: scheme.primary,
                            fontWeight: FontWeight.w600,
                            fontSize: 12,
                          ),
                        ),
                        Text(
                          widget.replyTo!.body,
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
                    onPressed: widget.onCancelReply,
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
                      root: widget.attachmentRoot,
                      onOpen: widget.onOpenAttachment == null
                          ? null
                          : () => widget.onOpenAttachment!(file),
                      onRemove: busy || widget.onRemoveAttachment == null
                          ? null
                          : () => widget.onRemoveAttachment!(file),
                    ),
                ],
              ),
            ),
          if (busy)
            Padding(
              padding: const EdgeInsets.all(8),
              child: Row(
                children: [
                  const SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(strokeWidth: 1.6),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      widget.busyLabel,
                      style: const TextStyle(fontSize: 12),
                    ),
                  ),
                ],
              ),
            ),
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              if (widget.onAttach != null || widget.mentions.isNotEmpty ||
                  (widget.mentionOptions?.isNotEmpty ?? false))
                PopupMenuButton<String>(
                  tooltip: desktop ? 'Add files or mention' : 'Add to message',
                  enabled: !busy,
                  icon: const Icon(Icons.add_rounded, size: 22),
                  itemBuilder: (_) => [
                    if (widget.onAttach != null)
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
                    if (widget.onAttach != null && _options.isNotEmpty)
                      const PopupMenuDivider(),
                    for (final option in _options)
                      PopupMenuItem(
                        value: '@${option.handle}',
                        child: Text(
                          option.everyone
                              ? '@all · everyone replies'
                              : '@${option.handle}',
                        ),
                      ),
                  ],
                  onSelected: (value) {
                    if (value == 'file') {
                      widget.onAttach?.call();
                      return;
                    }
                    _insertTag(value);
                    _focus.requestFocus();
                  },
                ),
              Expanded(
                child: CallbackShortcuts(
                  bindings: {
                    if (suggesting) ...{
                      const SingleActivator(LogicalKeyboardKey.arrowDown): () =>
                          _move(1),
                      const SingleActivator(LogicalKeyboardKey.arrowUp): () =>
                          _move(-1),
                      const SingleActivator(LogicalKeyboardKey.enter): () =>
                          _accept(_suggestions[_highlighted]),
                      const SingleActivator(LogicalKeyboardKey.tab): () =>
                          _accept(_suggestions[_highlighted]),
                      const SingleActivator(LogicalKeyboardKey.escape): () =>
                          setState(() {
                            _dismissedAt = _query?.start;
                            _suggestions = const [];
                          }),
                    } else if (widget.replyTo != null)
                      const SingleActivator(LogicalKeyboardKey.escape): () =>
                          widget.onCancelReply?.call(),
                    const SingleActivator(
                      LogicalKeyboardKey.enter,
                      control: true,
                    ): () {
                      if (!busy) widget.onSend();
                    },
                  },
                  child: TextField(
                    controller: controller,
                    focusNode: _focus,
                    enabled: !busy,
                    minLines: 1,
                    maxLines: 5,
                    style: const TextStyle(fontSize: 14, height: 1.5),
                    textInputAction: TextInputAction.send,
                    onSubmitted: (_) {
                      if (suggesting) return;
                      if (!busy &&
                          (controller.text.trim().isNotEmpty ||
                              attachments.isNotEmpty)) {
                        widget.onSend();
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
                        : widget.onSend,
                    style: FilledButton.styleFrom(
                      minimumSize: Size.square(desktop ? 36 : 44),
                      shape: desktop
                          ? RoundedRectangleBorder(
                              borderRadius: BorderRadius.circular(7),
                            )
                          : const CircleBorder(),
                      padding: EdgeInsets.all(desktop ? 8 : 12),
                      animationDuration: motionDuration(context, 120),
                    ),
                    child: Icon(
                      desktop ? Icons.send_rounded : Icons.arrow_upward_rounded,
                      size: desktop ? 17 : 20,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ],
      ),
    );
    final hint = Padding(
      padding: EdgeInsets.fromLTRB(desktop ? 30 : 22, 0, desktop ? 30 : 22, 8),
      child: Text(
        suggesting
            ? '↑ ↓ to choose · Enter or Tab to insert · Esc to close'
            : '@all asks every bot · @name asks one · Ctrl+Enter sends',
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
      ),
    );
    final column = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [composer, if (desktop && _options.isNotEmpty) hint else const SizedBox(height: 8)],
    );
    return desktop
        ? Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 968),
              child: column,
            ),
          )
        : column;
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
    // In a message, an image is shown properly, not as a stamp: a generated
    // picture is something to look at and reply to.
    if (preview && onRemove == null && file.mimeType.startsWith('image/')) {
      return Container(
        width: 264,
        margin: const EdgeInsets.only(right: 8, top: 6, bottom: 6),
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: scheme.surfaceContainerHigh,
          border: Border.all(color: scheme.outlineVariant),
          borderRadius: BorderRadius.circular(12),
        ),
        child: Tooltip(
          message: '${file.name}\n${file.note}',
          child: InkWell(
            onTap: onOpen,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: 264,
                  height: 176,
                  child: _AttachmentImage(path: '$root/${file.id}/image-0.jpg'),
                ),
                Padding(
                  padding: const EdgeInsets.fromLTRB(10, 6, 10, 8),
                  child: Text(
                    file.note.isNotEmpty ? '${file.name} · ${file.note}' : file.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    }
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

/// An attachment's image, checked before it is decoded.
class _AttachmentImage extends StatefulWidget {
  const _AttachmentImage({required this.path});

  final String path;

  @override
  State<_AttachmentImage> createState() => _AttachmentImageState();
}

class _AttachmentImageState extends State<_AttachmentImage> {
  late Future<Uint8List> _bytes = checkedImage(File(widget.path).readAsBytes());

  @override
  void didUpdateWidget(covariant _AttachmentImage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.path != widget.path) {
      _bytes = checkedImage(File(widget.path).readAsBytes());
    }
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return FutureBuilder<Uint8List>(
      future: _bytes,
      builder: (context, snapshot) {
        if (snapshot.hasData) {
          return Image.memory(
            snapshot.data!,
            fit: BoxFit.cover,
            cacheWidth: 528,
            errorBuilder: (_, _, _) =>
                Icon(Icons.broken_image_outlined, color: scheme.onSurfaceVariant),
          );
        }
        if (snapshot.hasError) {
          return Center(
            child: Text(
              'Preview not available',
              style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
            ),
          );
        }
        return ColoredBox(color: scheme.surfaceContainerHighest.withValues(alpha: 0.6));
      },
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
  const _FailureBanner({required this.failing, this.onDismiss});

  final List<BotActivity> failing;
  final VoidCallback? onDismiss;

  void _details(BuildContext context) => showModalBottomSheet<void>(
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
  );

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final worst = failing.first;
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 560),
      child: Material(
        color: scheme.surfaceContainerLow,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
          side: BorderSide(color: scheme.error.withValues(alpha: 0.35)),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: () => _details(context),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(12, 4, 4, 4),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.error_outline, size: 16, color: scheme.error),
                const SizedBox(width: 8),
                Flexible(
                  child: Text(
                    failing.length == 1
                        ? '@${worst.agent.handle} — previous turn failed'
                        : '${failing.length} bots have previous turn failures',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, color: scheme.onSurface),
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  'Details',
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: scheme.primary,
                  ),
                ),
                if (onDismiss != null)
                  IconButton(
                    tooltip: 'Hide until the next failure',
                    visualDensity: VisualDensity.compact,
                    iconSize: 16,
                    onPressed: onDismiss,
                    icon: const Icon(Icons.close),
                  )
                else
                  const SizedBox(width: 8),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
