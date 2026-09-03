// The models a bot can be pointed at.
//
// A free-text box asked people to know exact ids like `claude-opus-5` and gave
// no feedback when they got one wrong — the room would just fail on its first
// turn with an HTTP 400. This is a curated list per provider, with a "Custom"
// escape hatch so a new or private model is never locked out.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'models.dart';

/// One selectable model.
class ModelOption {
  /// The id sent to the API.
  final String id;

  /// What the picker shows.
  final String label;

  /// A one-line hint about when to reach for it.
  final String note;

  const ModelOption(this.id, this.label, this.note);
}

/// The curated models, per provider.
///
/// The first entry of each list is that provider's default: it is what a bot
/// gets when you have not chosen, and it is chosen to be the sensible pick
/// rather than the cheapest.
class ModelCatalog {
  static const List<ModelOption> anthropic = [
    ModelOption('claude-opus-5', 'Claude Opus 5', 'Best judgement — the default'),
    ModelOption('claude-sonnet-5', 'Claude Sonnet 5', 'Fast and capable, cheaper'),
    ModelOption('claude-haiku-4-5', 'Claude Haiku 4.5', 'Cheapest — good for inspectors'),
    ModelOption('claude-opus-4-8', 'Claude Opus 4.8', 'Previous Opus generation'),
  ];

  static const List<ModelOption> deepseek = [
    ModelOption('deepseek-chat', 'DeepSeek Chat', 'General purpose — the default'),
    ModelOption('deepseek-reasoner', 'DeepSeek Reasoner', 'Shows its reasoning; slower'),
  ];

  static const List<ModelOption> openai = [
    ModelOption('gpt-5.2', 'GPT-5.2', 'General purpose — the default'),
    ModelOption('gpt-5.2-codex', 'GPT-5.2 Codex', 'Tuned for code'),
    ModelOption('gpt-5-mini', 'GPT-5 mini', 'Cheaper and faster'),
  ];

  /// The options offered for a provider. Empty for the scripted demo bot,
  /// which has no model behind it.
  static List<ModelOption> forProvider(ProviderKind kind) {
    switch (kind) {
      case ProviderKind.anthropic:
        return anthropic;
      case ProviderKind.deepseek:
        return deepseek;
      case ProviderKind.openai:
        return openai;
      case ProviderKind.scripted:
        return const [];
    }
  }

  /// The id a bot uses when none has been chosen.
  static String defaultFor(ProviderKind kind) {
    final options = forProvider(kind);
    return options.isEmpty ? '' : options.first.id;
  }

  /// The display label for an id, falling back to the raw id for a custom one.
  static String labelFor(ProviderKind kind, String id) {
    if (id.isEmpty) {
      final fallback = defaultFor(kind);
      return fallback.isEmpty ? 'demo' : labelFor(kind, fallback) + ' (default)';
    }
    for (final option in forProvider(kind)) {
      if (option.id == id) return option.label;
    }
    return id;
  }

  /// Whether `id` is one of the curated options, as opposed to a custom entry.
  static bool isKnown(ProviderKind kind, String id) =>
      forProvider(kind).any((o) => o.id == id);
}
