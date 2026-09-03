// Maps a role's icon name to a real icon.
//
// This lives on the UI side so `roles.dart` can stay plain Dart and be used
// from tooling and tests without dragging in the widget toolkit.
library;

import 'package:flutter/material.dart';

const Map<String, IconData> _icons = {
  'hub_outlined': Icons.hub_outlined,
  'flag_outlined': Icons.flag_outlined,
  'architecture_outlined': Icons.architecture_outlined,
  'code': Icons.code,
  'web_outlined': Icons.web_outlined,
  'dns_outlined': Icons.dns_outlined,
  'brush_outlined': Icons.brush_outlined,
  'edit_outlined': Icons.edit_outlined,
  'query_stats': Icons.query_stats,
  'rate_review_outlined': Icons.rate_review_outlined,
  'bug_report_outlined': Icons.bug_report_outlined,
  'shield_outlined': Icons.shield_outlined,
  'spellcheck': Icons.spellcheck,
  'travel_explore_outlined': Icons.travel_explore_outlined,
  'insights_outlined': Icons.insights_outlined,
  'menu_book_outlined': Icons.menu_book_outlined,
  'settings_suggest_outlined': Icons.settings_suggest_outlined,
  'support_agent_outlined': Icons.support_agent_outlined,
  'all_inclusive': Icons.all_inclusive,
};

/// The icon for a role, falling back to a neutral one for an unknown name.
IconData roleIcon(String name) => _icons[name] ?? Icons.smart_toy_outlined;
