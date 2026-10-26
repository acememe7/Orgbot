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
        Theme.of(context).platform == TargetPlatform.linux
            ? 'assets/orgbots-desktop.png'
            : 'assets/logo.png',
        width: size,
        height: size,
        cacheWidth: (size * MediaQuery.devicePixelRatioOf(context)).ceil(),
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
  const StatusPill({
    super.key,
    required this.color,
    required this.label,
    this.icon,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final tone = color == Colors.blue || color == Colors.lightBlue
        ? scheme.secondary
        : color == Colors.green
        ? scheme.tertiary
        : color == Colors.red
        ? scheme.error
        : color == Colors.amber && scheme.brightness == Brightness.light
        ? const Color(0xFF8C6400)
        : color;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        color: tone.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(20),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 12, color: tone),
            const SizedBox(width: 5),
          ] else ...[
            Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(color: tone, shape: BoxShape.circle),
            ),
            const SizedBox(width: 6),
          ],
          Flexible(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                color: tone,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A stable colour per handle, so a bot reads the same everywhere.
Color handleColor(String handle, {bool dark = true}) {
  const palette = [
    Color(0xFF91ABA0),
    Color(0xFFA4A6B8),
    Color(0xFFBDA487),
    Color(0xFFB59B9D),
    Color(0xFF94A8BB),
    Color(0xFFA3AB8D),
  ];
  final color =
      palette[handle.isEmpty ? 0 : handle.hashCode.abs() % palette.length];
  return dark ? color : HSLColor.fromColor(color).withLightness(0.33).toColor();
}

/// An avatar for a bot, coloured by handle.
class BotAvatar extends StatelessWidget {
  final String handle;
  final double radius;
  const BotAvatar({super.key, required this.handle, this.radius = 18});

  @override
  Widget build(BuildContext context) {
    final color = handleColor(
      handle,
      dark: Theme.of(context).brightness == Brightness.dark,
    );
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
        child: SingleChildScrollView(
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
