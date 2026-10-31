// @mentions: finding the tag being typed, suggesting who it can address, and
// showing tags in sent messages so you can see who a message went to.
library;

import 'package:flutter/material.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:markdown/markdown.dart' as md;

import '../brand.dart';

/// Someone (or everyone) a message can be addressed to.
class MentionOption {
  const MentionOption({
    required this.handle,
    this.name = '',
    this.detail = '',
    this.everyone = false,
  });

  /// What is inserted after the `@`.
  final String handle;

  /// The display name, when it differs from the handle.
  final String name;

  /// A short line such as the bot's roles.
  final String detail;

  /// The room-wide tag. The engine wakes every bot for it.
  final bool everyone;

  static const all = MentionOption(
    handle: 'all',
    name: 'Everyone',
    detail: 'Every bot in the room replies',
    everyone: true,
  );

  bool matches(String query) {
    if (query.isEmpty) return true;
    final q = query.toLowerCase();
    if (handle.toLowerCase().startsWith(q)) return true;
    if (name.toLowerCase().split(RegExp(r'\s+')).any((w) => w.startsWith(q))) {
      return true;
    }
    return everyone && ('everyone'.startsWith(q) || 'everybody'.startsWith(q));
  }
}

/// The `@tag` being typed at the cursor, if any.
class MentionQuery {
  const MentionQuery(this.start, this.text);

  /// Offset of the `@`.
  final int start;

  /// What follows the `@` up to the cursor.
  final String text;

  static final _token = RegExp(r'(^|\s)@([\w-]*)$');

  static MentionQuery? at(TextEditingValue value) {
    final selection = value.selection;
    if (!selection.isValid || !selection.isCollapsed) return null;
    final cursor = selection.baseOffset;
    if (cursor < 0 || cursor > value.text.length) return null;
    final match = _token.firstMatch(value.text.substring(0, cursor));
    if (match == null) return null;
    final tag = match.group(2)!;
    return MentionQuery(cursor - tag.length - 1, tag);
  }

  /// [value] with this query replaced by `@handle `.
  TextEditingValue complete(TextEditingValue value, String handle) {
    final end = start + 1 + text.length;
    final rest = value.text.substring(end);
    final insert = rest.startsWith(' ') ? '@$handle' : '@$handle ';
    final next = value.text.replaceRange(start, end, insert);
    final cursor = start + insert.length + (rest.startsWith(' ') ? 1 : 0);
    return TextEditingValue(
      text: next,
      selection: TextSelection.collapsed(offset: cursor),
    );
  }
}

/// The list of people an `@` can reach, shown above the message field.
class MentionSuggestions extends StatelessWidget {
  const MentionSuggestions({
    super.key,
    required this.options,
    required this.highlighted,
    required this.onSelected,
    this.onHover,
  });

  final List<MentionOption> options;
  final int highlighted;
  final ValueChanged<MentionOption> onSelected;
  final ValueChanged<int>? onHover;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final dark = Theme.of(context).brightness == Brightness.dark;
    return ConstrainedBox(
      constraints: const BoxConstraints(maxHeight: 248),
      child: ListView.builder(
        key: const ValueKey('mention-suggestions'),
        shrinkWrap: true,
        padding: const EdgeInsets.symmetric(vertical: 4),
        itemCount: options.length,
        itemBuilder: (context, i) {
          final option = options[i];
          final selected = i == highlighted;
          final color = option.everyone
              ? scheme.primary
              : handleColor(option.handle, dark: dark);
          return MouseRegion(
            onEnter: (_) => onHover?.call(i),
            child: Material(
              color: selected
                  ? scheme.primary.withValues(alpha: dark ? 0.16 : 0.10)
                  : Colors.transparent,
              borderRadius: BorderRadius.circular(6),
              child: InkWell(
                borderRadius: BorderRadius.circular(6),
                onTap: () => onSelected(option),
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 10,
                    vertical: 7,
                  ),
                  child: Row(
                    children: [
                      if (option.everyone)
                        CircleAvatar(
                          radius: 11,
                          backgroundColor: color.withValues(alpha: 0.18),
                          child: Icon(
                            Icons.groups_2_outlined,
                            size: 14,
                            color: color,
                          ),
                        )
                      else
                        BotAvatar(handle: option.handle, radius: 11),
                      const SizedBox(width: 10),
                      Text(
                        '@${option.handle}',
                        style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                          color: scheme.onSurface,
                        ),
                      ),
                      if (option.name.isNotEmpty &&
                          option.name.toLowerCase() !=
                              option.handle.toLowerCase()) ...[
                        const SizedBox(width: 8),
                        Text(
                          option.name,
                          style: TextStyle(
                            fontSize: 13,
                            color: scheme.onSurface,
                          ),
                        ),
                      ],
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(
                          option.detail,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 12,
                            color: scheme.onSurfaceVariant,
                          ),
                        ),
                      ),
                      if (selected)
                        Text(
                          'Enter',
                          style: TextStyle(
                            fontSize: 11,
                            color: scheme.onSurfaceVariant,
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}

/// Parses `@handle` in message text so it can be drawn as a tag.
class MentionSyntax extends md.InlineSyntax {
  // Not preceded by a word character, so email addresses stay plain text.
  MentionSyntax() : super(r'(?<![\w@])@[A-Za-z0-9][\w-]*');

  @override
  bool onMatch(md.InlineParser parser, Match match) {
    parser.addNode(md.Element.text('mention', match[0]!));
    return true;
  }
}

/// Draws a parsed `@handle` in the tag style, merged into the paragraph.
class MentionBuilder extends MarkdownElementBuilder {
  MentionBuilder(this.style);

  final TextStyle style;

  @override
  Widget? visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) => Text.rich(
    TextSpan(
      text: element.textContent,
      style: parentStyle?.merge(style) ?? style,
    ),
  );
}

/// Who a message from the owner reaches, by the room's own rules: `@all`
/// (or `@everyone`, `@everybody`) reaches every bot; any other mention reaches
/// only the bots named; with none, a reply reaches the bot replied to and
/// anything else reaches one coordinator.
///
/// From the live room: an update meant for everyone mentioned two bots in
/// passing and reached only those two; nothing said so before it was sent.
String? messageAudience(
  String text,
  List<MentionOption> options, {
  String? replyHandle,
}) {
  if (text.trim().isEmpty) return null;
  final tags = text
      .split(RegExp(r'[^\p{L}\p{N}@_-]', unicode: true))
      .where((word) => word.startsWith('@') && word.length > 1)
      .map((word) => word.substring(1))
      .toList();
  const broadcast = {'all', 'everyone', 'everybody'};
  if (tags.any((tag) => broadcast.contains(tag.toLowerCase()))) {
    return 'Goes to every bot';
  }
  if (tags.isEmpty) {
    if (replyHandle != null) return 'Goes to @$replyHandle only, as a reply';
    return 'Goes to one coordinator only. Add @all to reach every bot';
  }
  final named = <String>[];
  for (final tag in tags) {
    final matches = options
        .where(
          (option) =>
              !option.everyone &&
              (option.handle.toLowerCase() == tag.toLowerCase() ||
                  (option.name.isNotEmpty &&
                      option.name.toLowerCase() == tag.toLowerCase())),
        )
        .toList();
    if (matches.length == 1 && !named.contains(matches.single.handle)) {
      named.add(matches.single.handle);
    }
  }
  if (named.isEmpty) return 'Goes to no bot: nobody here has that name';
  final who = named.length == 1
      ? '@${named.single}'
      : '${named.take(named.length - 1).map((h) => '@$h').join(', ')} and @${named.last}';
  return 'Goes to $who only. Add @all to reach every bot';
}
