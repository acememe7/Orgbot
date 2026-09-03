/// The chat transcript and the message composer.
library;

import 'package:flutter/material.dart';

import '../../protocol/models.dart';
import '../../state/room_store.dart';

/// The centre pane: the room transcript plus an input to talk to the room.
class ChatPane extends StatefulWidget {
  const ChatPane({super.key, required this.store, required this.displayName});

  final RoomStore store;
  final String displayName;

  @override
  State<ChatPane> createState() => _ChatPaneState();
}

class _ChatPaneState extends State<ChatPane> {
  final _input = TextEditingController();
  final _scroll = ScrollController();
  int _lastCount = 0;

  @override
  void dispose() {
    _input.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _send() {
    widget.store.send(widget.displayName, _input.text);
    _input.clear();
  }

  void _autoScroll() {
    // Follow the conversation only when new messages arrive, so a user
    // scrolling back to read is not yanked to the bottom.
    final count = widget.store.messages.length;
    if (count != _lastCount) {
      _lastCount = count;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scroll.hasClients) {
          _scroll.animateTo(
            _scroll.position.maxScrollExtent,
            duration: const Duration(milliseconds: 200),
            curve: Curves.easeOut,
          );
        }
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    _autoScroll();
    final messages = widget.store.messages;
    return Column(
      children: [
        Expanded(
          child: messages.isEmpty
              ? const _EmptyChat()
              : ListView.builder(
                  controller: _scroll,
                  padding: const EdgeInsets.symmetric(vertical: 12, horizontal: 16),
                  itemCount: messages.length,
                  itemBuilder: (context, i) => _MessageBubble(
                    message: messages[i],
                    store: widget.store,
                  ),
                ),
        ),
        const Divider(height: 1),
        _Composer(controller: _input, onSend: _send),
      ],
    );
  }
}

class _EmptyChat extends StatelessWidget {
  const _EmptyChat();

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.chat_bubble_outline, size: 40, color: scheme.onSurfaceVariant),
          const SizedBox(height: 12),
          Text('No messages yet', style: TextStyle(color: scheme.onSurfaceVariant)),
          const SizedBox(height: 4),
          Text(
            'Say something to get the room going',
            style: TextStyle(fontSize: 12, color: scheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

/// One message, styled by who sent it.
class _MessageBubble extends StatelessWidget {
  const _MessageBubble({required this.message, required this.store});

  final ChatMessage message;
  final RoomStore store;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final author = message.author;

    if (author.isSystem) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
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
    final name = isHuman ? (author.name ?? 'human') : store.handleFor(author.agentId);
    final color = isHuman ? scheme.primary : _agentColor(name, scheme);
    final isAsk = message.intent == 'ask';

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 5),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          CircleAvatar(
            radius: 15,
            backgroundColor: color.withValues(alpha: 0.2),
            child: Text(
              _initials(name),
              style: TextStyle(fontSize: 11, fontWeight: FontWeight.bold, color: color),
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Text(
                      isHuman ? name : '@$name',
                      style: TextStyle(fontWeight: FontWeight.w600, fontSize: 13, color: color),
                    ),
                    if (isAsk) ...[
                      const SizedBox(width: 6),
                      Icon(Icons.help_outline, size: 13, color: scheme.onSurfaceVariant),
                    ],
                  ],
                ),
                const SizedBox(height: 2),
                SelectableText(
                  message.body,
                  style: const TextStyle(fontSize: 14, height: 1.35),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  static String _initials(String name) {
    final clean = name.replaceAll('@', '');
    return clean.isEmpty ? '?' : clean.substring(0, 1).toUpperCase();
  }

  /// A stable colour per bot handle, so each bot reads consistently.
  static Color _agentColor(String name, ColorScheme scheme) {
    const palette = [
      Color(0xFF5AA9E6),
      Color(0xFF7FB800),
      Color(0xFFE6A15A),
      Color(0xFFE65A9B),
      Color(0xFF9B5AE6),
      Color(0xFF5AE6C3),
    ];
    return palette[name.hashCode.abs() % palette.length];
  }
}

/// The message input row.
class _Composer extends StatelessWidget {
  const _Composer({required this.controller, required this.onSend});

  final TextEditingController controller;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Row(
        children: [
          Expanded(
            child: TextField(
              controller: controller,
              minLines: 1,
              maxLines: 4,
              textInputAction: TextInputAction.send,
              onSubmitted: (_) => onSend(),
              decoration: InputDecoration(
                hintText: 'Message the room…',
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(24),
                ),
                contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
              ),
            ),
          ),
          const SizedBox(width: 8),
          FilledButton(
            onPressed: onSend,
            style: FilledButton.styleFrom(
              shape: const CircleBorder(),
              padding: const EdgeInsets.all(14),
            ),
            child: const Icon(Icons.send, size: 18),
          ),
        ],
      ),
    );
  }
}
