import 'package:flutter/material.dart';

import '../../state/room_recap.dart';

/// What happened in the room while the owner was away, until dismissed.
class RecapCard extends StatelessWidget {
  const RecapCard({super.key, required this.recap, required this.onDismiss});

  final RoomRecap recap;
  final VoidCallback onDismiss;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final text = Theme.of(context).textTheme;
    return Container(
      key: const ValueKey('room-recap'),
      width: double.infinity,
      margin: const EdgeInsets.fromLTRB(12, 8, 12, 4),
      padding: const EdgeInsets.fromLTRB(14, 10, 4, 10),
      decoration: BoxDecoration(
        color: scheme.secondaryContainer.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(recap.heading, style: text.titleSmall),
                const SizedBox(height: 4),
                for (final line in recap.lines)
                  Padding(
                    padding: const EdgeInsets.only(top: 2),
                    child: Text(line, style: text.bodySmall),
                  ),
              ],
            ),
          ),
          IconButton(
            tooltip: 'Dismiss',
            icon: const Icon(Icons.close, size: 18),
            onPressed: onDismiss,
          ),
        ],
      ),
    );
  }
}
