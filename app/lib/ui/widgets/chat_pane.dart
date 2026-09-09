/// The connected room uses the same transcript and composer as native rooms.
library;

import 'package:flutter/material.dart';
import '../../state/room_store.dart';
import '../room_widgets.dart';

class ChatPane extends StatefulWidget {
  const ChatPane({super.key, required this.store, required this.displayName});
  final RoomStore store;
  final String displayName;
  @override
  State<ChatPane> createState() => _ChatPaneState();
}

class _ChatPaneState extends State<ChatPane>
    with AutomaticKeepAliveClientMixin {
  final _input = TextEditingController();
  final _scroll = ScrollController();
  @override
  bool get wantKeepAlive => true;
  @override
  void dispose() {
    _input.dispose();
    _scroll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    super.build(context);
    return Column(
      children: [
        Expanded(
          child: Transcript(store: widget.store, controller: _scroll),
        ),
        Composer(
          controller: _input,
          onSend: () {
            if (_input.text.trim().isEmpty) return;
            widget.store.send(widget.displayName, _input.text);
            _input.clear();
          },
          mentions: [for (final bot in widget.store.agents) bot.handle],
        ),
      ],
    );
  }
}
