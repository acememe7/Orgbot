// What a person sees instead of the board.
//
// The old Work tab led with every record the room had made — ninety-six of
// them in a real run, for twelve actual outcomes — and the count went *up* as
// the room made progress, because finishing something produced a review and a
// review produced a remediation. The board got busier the closer it got to
// done.
//
// This leads with outcomes instead. Nothing is hidden: the full record list is
// one tap away under each objective, and the whole raw board is still a tap
// below. Compression is a UX layer, not deletion.
//
// The one thing given real visual weight is whether you have to do something.
// Every card says so outright, because not knowing was what made the old board
// exhausting to read.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:async';

import 'package:flutter/material.dart';

import '../../protocol/models.dart';
import '../../state/room_store.dart';
import '../polish.dart';

final Expando<_AnswerState> _answerStates = Expando<_AnswerState>();

_AnswerState _answersFor(RoomStore store) =>
    _answerStates[store] ??= _AnswerState();

class _AnswerState extends ChangeNotifier {
  final Set<String> pending = <String>{};
  final Set<String> retryReady = <String>{};
  final Map<String, Timer> _timers = <String, Timer>{};
  int _attachedPanels = 0;

  void attachPanel() {
    _attachedPanels++;
  }

  void detachPanel() {
    _attachedPanels--;
    if (_attachedPanels == 0) {
      for (final timer in _timers.values) {
        timer.cancel();
      }
      _timers.clear();
      retryReady.addAll(pending);
      pending.clear();
    }
  }

  bool begin(String id) {
    if (!pending.add(id)) return false;
    retryReady.remove(id);
    _timers[id] = Timer(const Duration(seconds: 12), () => _expire(id));
    notifyListeners();
    return true;
  }

  void _expire(String id) {
    _timers.remove(id);
    if (pending.remove(id)) {
      retryReady.add(id);
      notifyListeners();
    }
  }

  void settleAbsentFrom(Set<String> liveIds) {
    for (final id in pending.where((id) => !liveIds.contains(id)).toList()) {
      pending.remove(id);
      retryReady.remove(id);
      _timers.remove(id)?.cancel();
    }
    retryReady.removeWhere((id) => !liveIds.contains(id));
  }
}

/// Which slice of the room is being looked at.
///
/// `Needs me` shows only work genuinely waiting on a person — the whole point
/// of the split. Without it, a real decision sits among ninety-five records
/// that are all fine.
enum WorkView {
  overview('Overview'),
  active('Active'),
  needsMe('Needs me'),
  blocked('Blocked');

  const WorkView(this.label);
  final String label;
}

/// Objectives, progress, and the decisions only you can make.
class ObjectivesPane extends StatefulWidget {
  const ObjectivesPane({super.key, required this.store, this.onShowAllRecords});

  final RoomStore store;

  /// Opens the full task board. Null hides the affordance.
  final VoidCallback? onShowAllRecords;

  @override
  State<ObjectivesPane> createState() => _ObjectivesPaneState();
}

class _ObjectivesPaneState extends State<ObjectivesPane> {
  WorkView _view = WorkView.overview;

  /// The cards this view shows.
  List<ObjectiveCard> _visible(RoomDigest digest) => switch (_view) {
    WorkView.overview => digest.objectives,
    WorkView.active =>
      digest.objectives
          .where((c) => c.owners.isNotEmpty && !c.needsOperator)
          .toList(),
    WorkView.needsMe =>
      digest.objectives.where((c) => c.needsOperator).toList(),
    WorkView.blocked =>
      digest.objectives
          .where((c) => c.blocker != null && !c.needsOperator)
          .toList(),
  };

  @override
  Widget build(BuildContext context) {
    final store = widget.store;
    final onShowAllRecords = widget.onShowAllRecords;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final digest = store.digest;
        final scheme = Theme.of(context).colorScheme;

        if (digest.isEmpty) {
          return _Empty(
            hasTasks: store.tasks.isNotEmpty,
            onShowAllRecords: onShowAllRecords,
          );
        }

        final cards = _visible(digest);
        return ListView(
          padding: const EdgeInsets.fromLTRB(14, 14, 14, 28),
          children: [
            _Headline(counts: digest.counts),
            if (digest.needsYou.isNotEmpty && _view == WorkView.overview) ...[
              const SizedBox(height: 16),
              NeedsYouPanel(decisions: digest.needsYou, store: store),
            ],
            const SizedBox(height: 16),
            _ViewPicker(
              current: _view,
              counts: digest.counts,
              onPick: (view) => setState(() => _view = view),
            ),
            const SizedBox(height: 14),
            if (digest.needsYou.isNotEmpty && _view == WorkView.needsMe) ...[
              NeedsYouPanel(decisions: digest.needsYou, store: store),
              const SizedBox(height: 14),
            ],
            if (cards.isEmpty &&
                !(_view == WorkView.needsMe && digest.needsYou.isNotEmpty))
              _NothingHere(view: _view)
            else if (cards.isNotEmpty)
              for (final card in cards)
                _ObjectiveTile(card: card, store: store),
            if (_view == WorkView.overview) ...[
              if (digest.criticalPath.isNotEmpty) ...[
                const SizedBox(height: 16),
                _CriticalPath(digest: digest, store: store),
              ],
              const SizedBox(height: 12),
              _UtilizationStrip(utilization: digest.utilization),
              if (digest.spend.isNotEmpty) ...[
                const SizedBox(height: 14),
                _SpendStrip(spend: digest.spend),
              ],
              if (digest.health.coordinationExpansion) ...[
                const SizedBox(height: 16),
                _ExpansionWarning(health: digest.health),
              ],
            ],
            const SizedBox(height: 18),
            if (onShowAllRecords != null)
              Center(
                child: TextButton.icon(
                  onPressed: onShowAllRecords,
                  icon: const Icon(Icons.list_alt_outlined, size: 18),
                  label: Text(
                    'Show all ${digest.counts.rawRecords} work records',
                  ),
                  style: TextButton.styleFrom(
                    foregroundColor: scheme.onSurfaceVariant,
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

/// The counts, in the order they matter — and never the raw record total.
class _Headline extends StatelessWidget {
  const _Headline({required this.counts});

  final BoardCounts counts;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final quiet = counts.needsYou == 0;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: scheme.outlineVariant),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              _Count(
                value: '${counts.delivered}/${counts.objectives}',
                label: 'objectives done',
              ),
              if (counts.active > 0)
                _Count(value: '${counts.active}', label: 'in flight'),
              if (counts.waitingInternally > 0)
                _Count(
                  value: '${counts.waitingInternally}',
                  label: 'waiting on the room',
                ),
            ],
          ),
          const SizedBox(height: 14),
          Row(
            children: [
              Icon(
                quiet ? Icons.check_circle_outline : Icons.pending_actions,
                size: 17,
                color: quiet ? Colors.green : scheme.error,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  quiet
                      ? 'Nothing needs you.'
                      : counts.needsYou == 1
                      ? '1 decision needs you.'
                      : '${counts.needsYou} decisions need you.',
                  style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                    color: quiet ? scheme.onSurfaceVariant : scheme.error,
                  ),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Count extends StatelessWidget {
  const _Count({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Expanded(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            value,
            style: const TextStyle(
              fontSize: 22,
              fontWeight: FontWeight.w700,
              height: 1.1,
            ),
          ),
          const SizedBox(height: 2),
          Text(
            label,
            style: TextStyle(fontSize: 11, color: scheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

/// The operator's inbox: only things a person must actually answer.
class NeedsYouPanel extends StatefulWidget {
  const NeedsYouPanel({
    super.key,
    required this.decisions,
    required this.store,
    this.compact = false,
    this.focusNode,
  });

  final List<PendingDecision> decisions;
  final RoomStore store;
  final bool compact;
  final FocusNode? focusNode;

  @override
  State<NeedsYouPanel> createState() => _NeedsYouPanelState();
}

class _NeedsYouPanelState extends State<NeedsYouPanel> {
  final Map<String, TextEditingController> _responses = {};
  late _AnswerState _answerState;

  @override
  void initState() {
    super.initState();
    _answerState = _answersFor(widget.store)
      ..attachPanel()
      ..addListener(_onAnswerStateChanged);
  }

  void _onAnswerStateChanged() {
    if (mounted) setState(() {});
  }

  @override
  void didUpdateWidget(covariant NeedsYouPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    final liveIds = widget.decisions.map((decision) => decision.id).toSet();
    if (oldWidget.store != widget.store) {
      _answerState.removeListener(_onAnswerStateChanged);
      _answerState.detachPanel();
      _answerState = _answersFor(widget.store)
        ..attachPanel()
        ..addListener(_onAnswerStateChanged);
    }
    _answerState.settleAbsentFrom(liveIds);
    for (final id in _responses.keys.toList()) {
      if (!liveIds.contains(id)) {
        _responses.remove(id)?.dispose();
      }
    }
  }

  @override
  void dispose() {
    _answerState.removeListener(_onAnswerStateChanged);
    _answerState.detachPanel();
    for (final response in _responses.values) {
      response.dispose();
    }
    super.dispose();
  }

  void _answer(PendingDecision decision, String chosen, {String note = ''}) {
    if (!_answerState.begin(decision.id)) return;
    widget.store.answerDecision(decision.id, chosen, note: note);
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final liveIds = widget.decisions.map((decision) => decision.id).toSet();
    _answerState.settleAbsentFrom(liveIds);
    if (widget.decisions.isEmpty) return const SizedBox.shrink();

    final body = Container(
      padding: EdgeInsets.all(widget.compact ? 11 : 14),
      decoration: BoxDecoration(
        color: scheme.primaryContainer.withValues(alpha: 0.42),
        borderRadius: BorderRadius.circular(widget.compact ? 10 : 14),
        border: Border.all(color: scheme.primary.withValues(alpha: 0.28)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.help_outline, size: 16, color: scheme.primary),
              const SizedBox(width: 8),
              Text(
                'Needs you',
                style: TextStyle(
                  fontWeight: FontWeight.w700,
                  fontSize: 13,
                  color: scheme.primary,
                ),
              ),
            ],
          ),
          for (final decision in widget.decisions) ...[
            const SizedBox(height: 10),
            _DecisionAnswerCard(
              key: ValueKey(decision.id),
              decision: decision,
              response: _responses.putIfAbsent(
                decision.id,
                TextEditingController.new,
              ),
              busy: _answerState.pending.contains(decision.id),
              retryReady: _answerState.retryReady.contains(decision.id),
              onAnswer: (chosen, note) => _answer(decision, chosen, note: note),
            ),
          ],
        ],
      ),
    );

    return AnimatedSize(
      duration: motionDuration(context, 160),
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: widget.compact
          ? ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 250),
              child: SingleChildScrollView(
                child: Focus(focusNode: widget.focusNode, child: body),
              ),
            )
          : Focus(focusNode: widget.focusNode, child: body),
    );
  }
}

class _DecisionAnswerCard extends StatelessWidget {
  const _DecisionAnswerCard({
    super.key,
    required this.decision,
    required this.response,
    required this.busy,
    required this.retryReady,
    required this.onAnswer,
  });

  final PendingDecision decision;
  final TextEditingController response;
  final bool busy;
  final bool retryReady;
  final void Function(String chosen, String note) onAnswer;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (decision.task.isNotEmpty) ...[
          Text(
            decision.title,
            style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
          ),
          const SizedBox(height: 3),
        ],
        Text(
          decision.because,
          style: const TextStyle(fontSize: 12.5, height: 1.35),
        ),
        if (decision.options.isNotEmpty) ...[
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 6,
            children: [
              for (final option in decision.options)
                OutlinedButton(
                  onPressed: busy ? null : () => onAnswer(option, ''),
                  style: OutlinedButton.styleFrom(
                    visualDensity: VisualDensity.compact,
                  ),
                  child: Text(option),
                ),
            ],
          ),
        ],
        const SizedBox(height: 8),
        ValueListenableBuilder<TextEditingValue>(
          valueListenable: response,
          builder: (context, value, _) => Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(
                child: TextField(
                  controller: response,
                  enabled: !busy,
                  maxLength: 1000,
                  minLines: 1,
                  maxLines: 3,
                  decoration: InputDecoration(
                    labelText: 'Custom answer',
                    isDense: true,
                    counterText: '',
                    border: const OutlineInputBorder(),
                    focusedBorder: OutlineInputBorder(
                      borderSide: BorderSide(color: scheme.primary),
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 8),
              FilledButton(
                onPressed: busy || value.text.trim().isEmpty
                    ? null
                    : () => onAnswer('Custom answer', value.text.trim()),
                child: Text(busy ? 'Sending…' : 'Send'),
              ),
            ],
          ),
        ),
        if (busy || retryReady)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              retryReady
                  ? 'No update arrived. Choose an answer to retry.'
                  : 'Answer sent. Waiting for the room to update.',
              style: TextStyle(fontSize: 11.5, color: scheme.onSurfaceVariant),
            ),
          ),
      ],
    );
  }
}

/// One outcome, with its records folded away until asked for.
class _ObjectiveTile extends StatefulWidget {
  const _ObjectiveTile({required this.card, required this.store});

  final ObjectiveCard card;
  final RoomStore store;

  @override
  State<_ObjectiveTile> createState() => _ObjectiveTileState();
}

class _ObjectiveTileState extends State<_ObjectiveTile> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final card = widget.card;
    final fraction = card.progress.fraction;

    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(13),
        border: Border.all(
          color: card.needsOperator ? scheme.error : scheme.outlineVariant,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 13, 14, 0),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  card.title,
                  style: const TextStyle(
                    fontWeight: FontWeight.w700,
                    fontSize: 14.5,
                  ),
                ),
                const SizedBox(height: 7),
                if (fraction != null) ...[
                  ClipRRect(
                    borderRadius: BorderRadius.circular(3),
                    child: LinearProgressIndicator(
                      value: fraction,
                      minHeight: 5,
                      backgroundColor: scheme.outlineVariant,
                    ),
                  ),
                  const SizedBox(height: 6),
                ],
                Row(
                  children: [
                    Text(
                      card.progress.label,
                      style: TextStyle(
                        fontSize: 11.5,
                        color: scheme.onSurfaceVariant,
                      ),
                    ),
                    const Spacer(),
                    _StatusChip(
                      status: card.status,
                      needsOperator: card.needsOperator,
                    ),
                  ],
                ),
                if (card.blocker != null) ...[
                  const SizedBox(height: 8),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(
                        card.needsOperator
                            ? Icons.pan_tool_outlined
                            : Icons.hourglass_empty,
                        size: 14,
                        color: card.needsOperator
                            ? scheme.error
                            : scheme.onSurfaceVariant,
                      ),
                      const SizedBox(width: 7),
                      Expanded(
                        child: Text(
                          card.blocker!,
                          style: TextStyle(
                            fontSize: 12,
                            height: 1.3,
                            color: card.needsOperator
                                ? scheme.error
                                : scheme.onSurfaceVariant,
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
                if (card.owners.isNotEmpty) ...[
                  const SizedBox(height: 7),
                  Text(
                    card.owners
                        .map((id) => '@' + widget.store.handleFor(id))
                        .join(', '),
                    style: TextStyle(
                      fontSize: 11.5,
                      color: scheme.onSurfaceVariant,
                    ),
                  ),
                ],
                // Said outright on every card. Not knowing this was the thing
                // that made the old board tiring.
                const SizedBox(height: 7),
                Text(
                  card.needsOperator ? 'Needs you: YES' : 'Needs you: no',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.4,
                    color: card.needsOperator
                        ? scheme.error
                        : scheme.onSurfaceVariant.withValues(alpha: 0.75),
                  ),
                ),
              ],
            ),
          ),
          if (card.records.isNotEmpty)
            TextButton(
              onPressed: () => setState(() => _open = !_open),
              style: TextButton.styleFrom(
                visualDensity: VisualDensity.compact,
                foregroundColor: scheme.onSurfaceVariant,
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(_open ? Icons.expand_less : Icons.expand_more, size: 17),
                  const SizedBox(width: 5),
                  Text(
                    _open
                        ? 'Hide the work behind it'
                        : '${card.records.length} records behind it',
                    style: const TextStyle(fontSize: 12),
                  ),
                ],
              ),
            ),
          if (_open)
            Padding(
              padding: const EdgeInsets.fromLTRB(14, 0, 14, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  for (final id in card.records)
                    _RecordLine(id: id, store: widget.store),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// One record beneath an objective, with what it is waiting on.
///
/// The expanded view is the graph: each record shows the edge out of it, so a
/// chain reads as a chain rather than as a flat list that happens to be in
/// order. Drawing it as an indented tree rather than a canvas keeps it legible
/// on a phone and readable by a screen reader.
class _RecordLine extends StatelessWidget {
  const _RecordLine({required this.id, required this.store});

  final String id;
  final RoomStore store;

  Task? _task(String id) =>
      store.tasks.where((t) => t.id == id).cast<Task?>().firstOrNull;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final task = _task(id);
    final handoff = store.handoffs
        .where(
          (h) =>
              h.taskId == id && (h.state == 'offered' || h.state == 'accepted'),
        )
        .cast<Handoff?>()
        .firstOrNull;

    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Container(
                  width: 5,
                  height: 5,
                  decoration: BoxDecoration(
                    color: scheme.onSurfaceVariant,
                    shape: BoxShape.circle,
                  ),
                ),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  task?.title ?? id,
                  style: TextStyle(
                    fontSize: 12,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
              ),
            ],
          ),
          // The edge out of this node.
          if (task != null)
            for (final upstream in task.dependsOn)
              Padding(
                padding: const EdgeInsets.only(left: 14, top: 3),
                child: Text(
                  '↳ waits on ${_task(upstream)?.title ?? upstream}',
                  style: TextStyle(
                    fontSize: 11.5,
                    color: scheme.onSurfaceVariant.withValues(alpha: 0.8),
                  ),
                ),
              ),
          // Delegation nobody has picked up looks exactly like work in
          // progress unless something says otherwise.
          if (handoff != null)
            Padding(
              padding: const EdgeInsets.only(left: 14, top: 3),
              child: Text(
                '↳ handed off — ${handoff.progress}',
                style: TextStyle(
                  fontSize: 11.5,
                  color: handoff.awaitingAcceptance
                      ? Colors.amber
                      : scheme.onSurfaceVariant.withValues(alpha: 0.8),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _StatusChip extends StatelessWidget {
  const _StatusChip({required this.status, required this.needsOperator});

  final String status;
  final bool needsOperator;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final tone = needsOperator ? scheme.error : scheme.primary;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: tone.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(20),
      ),
      child: Text(
        status,
        style: TextStyle(
          fontSize: 10.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.3,
          color: tone,
        ),
      ),
    );
  }
}

/// The room is producing more coordination than work.
class _ExpansionWarning extends StatelessWidget {
  const _ExpansionWarning({required this.health});

  final RoomHealth health;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.all(13),
      decoration: BoxDecoration(
        color: Colors.orange.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: Colors.orange.withValues(alpha: 0.4)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(Icons.trending_up, size: 16, color: Colors.orange),
          const SizedBox(width: 9),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Coordination expansion',
                  style: TextStyle(fontWeight: FontWeight.w700, fontSize: 12.5),
                ),
                const SizedBox(height: 3),
                Text(
                  'The room is keeping '
                  '${health.inflationRatio.toStringAsFixed(1)} records per '
                  'objective, ${health.supportingRecords} of them reviews and '
                  'follow-ups. It may be describing its work more than doing '
                  'it.',
                  style: TextStyle(
                    fontSize: 12,
                    height: 1.35,
                    color: scheme.onSurfaceVariant,
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

/// Before the room has any outcomes to show.
class _Empty extends StatelessWidget {
  const _Empty({required this.hasTasks, this.onShowAllRecords});

  final bool hasTasks;
  final VoidCallback? onShowAllRecords;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return LayoutBuilder(
      builder: (context, constraints) => SingleChildScrollView(
        padding: const EdgeInsets.all(32),
        child: ConstrainedBox(
          constraints: BoxConstraints(
            minHeight:
                constraints.maxHeight.isFinite && constraints.maxHeight > 64
                ? constraints.maxHeight - 64
                : 0,
          ),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                Icons.flag_outlined,
                size: 34,
                color: scheme.onSurfaceVariant,
              ),
              const SizedBox(height: 14),
              Text(
                hasTasks ? 'No objectives yet' : 'Nothing on the board yet',
                style: const TextStyle(
                  fontWeight: FontWeight.w700,
                  fontSize: 14,
                ),
              ),
              const SizedBox(height: 6),
              Text(
                hasTasks
                    ? 'The room is working, but has not grouped its work into '
                          'outcomes yet.'
                    : 'Objectives appear here as the room takes on work.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: 12.5,
                  height: 1.4,
                  color: scheme.onSurfaceVariant,
                ),
              ),
              if (hasTasks && onShowAllRecords != null) ...[
                const SizedBox(height: 14),
                TextButton.icon(
                  onPressed: onShowAllRecords,
                  icon: const Icon(Icons.list_alt_outlined, size: 18),
                  label: const Text('Show the full board'),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// Switching between slices of the room.
class _ViewPicker extends StatelessWidget {
  const _ViewPicker({
    required this.current,
    required this.counts,
    required this.onPick,
  });

  final WorkView current;
  final BoardCounts counts;
  final ValueChanged<WorkView> onPick;

  /// How many cards each view holds, so the labels are informative rather
  /// than a set of identical tabs.
  int _count(WorkView view) => switch (view) {
    WorkView.overview => counts.objectives,
    WorkView.active => counts.active,
    WorkView.needsMe => counts.needsYou,
    WorkView.blocked => counts.waitingInternally,
  };

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Row(
        children: [
          for (final view in WorkView.values)
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: _Chip(
                label: '${view.label} ${_count(view)}',
                selected: view == current,
                // The one view that means something is expected of you gets
                // the only colour on the row.
                tone: view == WorkView.needsMe && counts.needsYou > 0
                    ? scheme.error
                    : null,
                onTap: () => onPick(view),
              ),
            ),
        ],
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({
    required this.label,
    required this.selected,
    required this.onTap,
    this.tone,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;
  final Color? tone;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final colour = tone ?? scheme.primary;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(20),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 7),
        decoration: BoxDecoration(
          color: selected
              ? colour.withValues(alpha: 0.16)
              : scheme.surfaceContainerHighest.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: selected ? colour : scheme.outlineVariant),
        ),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 12,
            fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
            color: selected ? colour : scheme.onSurfaceVariant,
          ),
        ),
      ),
    );
  }
}

/// An empty slice, explained rather than left blank.
class _NothingHere extends StatelessWidget {
  const _NothingHere({required this.view});

  final WorkView view;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (icon, line) = switch (view) {
      WorkView.overview => (Icons.inbox_outlined, 'No objectives yet.'),
      WorkView.active => (
        Icons.pause_circle_outline,
        'Nothing is being worked on right now.',
      ),
      // The good case, and worth saying warmly rather than as an absence.
      WorkView.needsMe => (
        Icons.check_circle_outline,
        'Nothing needs you. The room is getting on with it.',
      ),
      WorkView.blocked => (
        Icons.check_circle_outline,
        'Nothing is waiting on anything.',
      ),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 30),
      child: Column(
        children: [
          Icon(icon, size: 28, color: scheme.onSurfaceVariant),
          const SizedBox(height: 10),
          Text(
            line,
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 13, color: scheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

/// The chain that decides when the room finishes.
///
/// More useful than any count: it is the sequence to unblock first, and it is
/// invisible in a flat list however carefully that list is sorted.
class _CriticalPath extends StatelessWidget {
  const _CriticalPath({required this.digest, required this.store});

  final RoomDigest digest;
  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final titles = digest.criticalPath
        .map(
          (id) =>
              store.tasks
                  .where((t) => t.id == id)
                  .map((t) => t.title)
                  .firstOrNull ??
              id,
        )
        .toList();

    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(13),
        border: Border.all(color: scheme.outlineVariant),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.timeline, size: 15, color: scheme.primary),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  'What decides when this finishes',
                  style: TextStyle(
                    fontWeight: FontWeight.w700,
                    fontSize: 12.5,
                    color: scheme.primary,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          for (var i = 0; i < titles.length; i++)
            Padding(
              padding: const EdgeInsets.only(bottom: 4),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 18,
                    child: Text(
                      i == 0 ? '→' : '↓',
                      style: TextStyle(
                        fontSize: 12,
                        color: i == 0
                            ? scheme.primary
                            : scheme.onSurfaceVariant,
                      ),
                    ),
                  ),
                  Expanded(
                    child: Text(
                      titles[i],
                      style: TextStyle(
                        fontSize: 12.5,
                        height: 1.3,
                        // The head of the chain is the one to unblock.
                        fontWeight: i == 0 ? FontWeight.w700 : FontWeight.w400,
                        color: i == 0 ? null : scheme.onSurfaceVariant,
                      ),
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

/// Where the room's effort is going.
/// What the room has drawn, by account.
///
/// Turns were always on the board and tokens were not, which meant the only
/// visible cost of raising concurrency was that the room went faster. A
/// roster mostly on one subscription draws on one allowance, and drawing on
/// it four times faster is not visible anywhere until the allowance is gone.
class _SpendStrip extends StatelessWidget {
  const _SpendStrip({required this.spend});

  final List<AccountSpend> spend;

  /// Group digits: a seven-figure token count is unreadable otherwise.
  static String _grouped(int value) {
    final digits = value.toString();
    final out = StringBuffer();
    for (var i = 0; i < digits.length; i++) {
      if (i > 0 && (digits.length - i) % 3 == 0) out.write(',');
      out.write(digits[i]);
    }
    return out.toString();
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          'What this has drawn',
          style: TextStyle(
            fontSize: 11,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.4,
            color: scheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: 6),
        for (final account in spend)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 2),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    account.backend,
                    style: const TextStyle(fontSize: 12),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                Text(
                  '${account.turns} turns',
                  style: TextStyle(
                    fontSize: 12,
                    color: scheme.onSurfaceVariant,
                  ),
                ),
                const SizedBox(width: 10),
                Text(
                  // Zero is "this backend does not report usage", not "free".
                  account.isMetered
                      ? '${_grouped(account.tokens)} tokens'
                      : 'not metered',
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: account.isMetered
                        ? scheme.onSurface
                        : scheme.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

class _UtilizationStrip extends StatelessWidget {
  const _UtilizationStrip({required this.utilization});

  final Utilization utilization;

  Color _tone(ColorScheme scheme, String doing) => switch (doing) {
    'working' => scheme.primary,
    'reviewing' => Colors.purple,
    'waiting' => Colors.amber,
    'faulted' => scheme.error,
    'offline' => scheme.outline,
    _ => scheme.onSurfaceVariant,
  };

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    if (utilization.bots.isEmpty) return const SizedBox.shrink();

    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(13),
        border: Border.all(
          color: utilization.isWastingCapacity
              ? Colors.amber.withValues(alpha: 0.5)
              : scheme.outlineVariant,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Who is doing what',
            style: TextStyle(
              fontWeight: FontWeight.w700,
              fontSize: 12.5,
              color: scheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 7,
            runSpacing: 7,
            children: [
              for (final bot in utilization.bots)
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 9,
                    vertical: 5,
                  ),
                  decoration: BoxDecoration(
                    color: _tone(scheme, bot.doing).withValues(alpha: 0.13),
                    borderRadius: BorderRadius.circular(7),
                  ),
                  child: Text(
                    '@${bot.handle} · ${bot.doing}',
                    style: TextStyle(
                      fontSize: 11,
                      color: _tone(scheme, bot.doing),
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
            ],
          ),
          // Two different faults that look identical from the counts alone,
          // and have opposite fixes. Naming the wrong one sends someone
          // looking in the wrong place.
          if (utilization.isShortHanded) ...[
            const SizedBox(height: 10),
            Text(
              'Nobody free can do ${utilization.starvedCount} of the open '
              'task(s): they need '
              '${utilization.starvedRoles.map((e) => '${e.role} (${e.tasks})').join(', ')}, '
              'and every bot holding those roles is busy. Add one, or let '
              'another role stand in.',
              style: TextStyle(
                fontSize: 11.5,
                height: 1.35,
                color: Colors.amber,
              ),
            ),
          ],
          if (utilization.isWastingCapacity) ...[
            const SizedBox(height: 10),
            Text(
              '${utilization.idle} free while work they could take sits '
              'unclaimed.',
              style: TextStyle(
                fontSize: 11.5,
                height: 1.35,
                color: scheme.onSurfaceVariant,
              ),
            ),
          ],
        ],
      ),
    );
  }
}
