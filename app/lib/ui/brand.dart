// Shared brand pieces, so every screen shows the same mark and the same
// vocabulary instead of each one inventing its own header.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'package:flutter/material.dart';

/// The OrgBot mark, from the bundled asset.
class BrandMark extends StatelessWidget {
  final double size;
  const BrandMark({super.key, this.size = 40});

  @override
  Widget build(BuildContext context) {
    return ClipRRect(
      borderRadius: BorderRadius.circular(size * 0.24),
      child: Image.asset(
        'assets/logo.png',
        width: size,
        height: size,
        filterQuality: FilterQuality.medium,
        // A missing asset must not take the screen down; fall back to the
        // icon so the layout still holds.
        errorBuilder: (context, _, _) => Icon(Icons.smart_toy, size: size),
      ),
    );
  }
}

/// A status dot with a label, used for bots and rooms alike.
class StatusPill extends StatelessWidget {
  final Color color;
  final String label;
  final IconData? icon;
  const StatusPill({super.key, required this.color, required this.label, this.icon});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(20),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 12, color: color),
            const SizedBox(width: 5),
          ] else ...[
            Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            ),
            const SizedBox(width: 6),
          ],
          Text(
            label,
            style: TextStyle(
                fontSize: 11, fontWeight: FontWeight.w600, color: color),
          ),
        ],
      ),
    );
  }
}

/// A stable colour per handle, so a bot reads the same everywhere.
Color handleColor(String handle) {
  const palette = [
    Color(0xFF5AA9E6),
    Color(0xFF7FB800),
    Color(0xFFE6A15A),
    Color(0xFFE65A9B),
    Color(0xFF9B5AE6),
    Color(0xFF2EC4B6),
  ];
  if (handle.isEmpty) return palette.first;
  return palette[handle.hashCode.abs() % palette.length];
}

/// An avatar for a bot, coloured by handle.
class BotAvatar extends StatelessWidget {
  final String handle;
  final double radius;
  const BotAvatar({super.key, required this.handle, this.radius = 18});

  @override
  Widget build(BuildContext context) {
    final color = handleColor(handle);
    final initial = handle.isEmpty ? '?' : handle[0].toUpperCase();
    return CircleAvatar(
      radius: radius,
      backgroundColor: color.withValues(alpha: 0.2),
      child: Text(
        initial,
        style: TextStyle(
          color: color,
          fontWeight: FontWeight.bold,
          fontSize: radius * 0.85,
        ),
      ),
    );
  }
}

/// A friendly empty state: an icon, a line of what is missing, and the action.
class EmptyState extends StatelessWidget {
  final IconData icon;
  final String title;
  final String message;
  final String? actionLabel;
  final VoidCallback? onAction;

  const EmptyState({
    super.key,
    required this.icon,
    required this.title,
    required this.message,
    this.actionLabel,
    this.onAction,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 340),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 44, color: scheme.onSurfaceVariant),
              const SizedBox(height: 14),
              Text(title, style: Theme.of(context).textTheme.titleMedium),
              const SizedBox(height: 6),
              Text(
                message,
                textAlign: TextAlign.center,
                style: TextStyle(color: scheme.onSurfaceVariant, height: 1.4),
              ),
              if (actionLabel != null && onAction != null) ...[
                const SizedBox(height: 18),
                FilledButton.icon(
                  onPressed: onAction,
                  icon: const Icon(Icons.add),
                  label: Text(actionLabel!),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
