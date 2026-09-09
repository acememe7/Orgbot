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

import 'package:flutter/material.dart';

import '../../protocol/models.dart';
import '../../state/room_store.dart';

/// Objectives, progress, and the decisions only you can make.
class ObjectivesPane extends StatelessWidget {
  const ObjectivesPane({
    super.key,
    required this.store,
    this.onShowAllRecords,
  });

  final RoomStore store;

  /// Opens the full task board. Null hides the affordance.
  final VoidCallback? onShowAllRecords;

  @override
  Widget build(BuildContext context) {
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

        return ListView(
          padding: const EdgeInsets.fromLTRB(14, 14, 14, 28),
          children: [
            _Headline(counts: digest.counts),
            if (digest.needsYou.isNotEmpty) ...[
              const SizedBox(height: 16),
              _NeedsYou(decisions: digest.needsYou),
            ],
            const SizedBox(height: 18),
            Text(
              digest.counts.objectives == 1 ? 'Objective' : 'Objectives',
              style: Theme.of(context).textTheme.labelLarge,
            ),
            const SizedBox(height: 8),
            for (final card in digest.objectives)
              _ObjectiveTile(card: card, store: store),
            if (digest.health.coordinationExpansion) ...[
              const SizedBox(height: 16),
              _ExpansionWarning(health: digest.health),
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
class _NeedsYou extends StatelessWidget {
  const _NeedsYou({required this.decisions});

  final List<PendingDecision> decisions;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: scheme.errorContainer.withValues(alpha: 0.35),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: scheme.error.withValues(alpha: 0.4)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.pan_tool_outlined, size: 16, color: scheme.error),
              const SizedBox(width: 8),
              Text(
                'Needs you',
                style: TextStyle(
                  fontWeight: FontWeight.w700,
                  fontSize: 13,
                  color: scheme.error,
                ),
              ),
            ],
          ),
          for (final decision in decisions) ...[
            const SizedBox(height: 12),
            Text(
              decision.title,
              style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
            ),
            const SizedBox(height: 3),
            Text(
              // The sentence the room had to be able to write before it was
              // allowed to interrupt anyone.
              'Decision required because: ' + decision.because,
              style: const TextStyle(fontSize: 12.5, height: 1.35),
            ),
            if (decision.options.isNotEmpty) ...[
              const SizedBox(height: 9),
              Wrap(
                spacing: 8,
                runSpacing: 6,
                children: [
                  for (final option in decision.options)
                    OutlinedButton(
                      onPressed: null,
                      style: OutlinedButton.styleFrom(
                        visualDensity: VisualDensity.compact,
                      ),
                      child: Text(option),
                    ),
                ],
              ),
              const SizedBox(height: 2),
              Text(
                'Answer in chat for now.',
                style: TextStyle(
                  fontSize: 11,
                  color: scheme.onSurfaceVariant,
                ),
              ),
            ],
          ],
        ],
      ),
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
                  card.needsOperator
                      ? 'Needs you: YES'
                      : 'Needs you: no',
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
                  for (final id in card.records) _RecordLine(id: id, store: widget.store),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// One record beneath an objective, in the expanded view.
class _RecordLine extends StatelessWidget {
  const _RecordLine({required this.id, required this.store});

  final String id;
  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final task =
        store.tasks.where((t) => t.id == id).cast<Task?>().firstOrNull;
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(
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
              style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
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
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.flag_outlined, size: 34, color: scheme.onSurfaceVariant),
            const SizedBox(height: 12),
            Text(
              hasTasks
                  ? 'No objectives yet'
                  : 'Nothing on the board yet',
              style: const TextStyle(fontWeight: FontWeight.w700, fontSize: 14),
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
    );
  }
}
