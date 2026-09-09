import 'dart:io';
import 'package:flutter/material.dart';
import '../state/room_archive.dart';

class RoomHistoryScreen extends StatefulWidget {
  final String roomId, roomName;
  const RoomHistoryScreen({
    super.key,
    required this.roomId,
    required this.roomName,
  });
  @override
  State<RoomHistoryScreen> createState() => _RoomHistoryScreenState();
}

class _RoomHistoryScreenState extends State<RoomHistoryScreen> {
  List<File> _files = [];
  File? _selected;
  String _transcript = 'Loading saved runs…';
  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final files = await RoomArchive.list(widget.roomId);
      if (!mounted) return;
      setState(() => _files = files);
      if (files.isEmpty) {
        setState(() => _transcript = 'No saved runs yet.');
      } else {
        await _read(files.first);
      }
    } catch (e) {
      if (mounted) setState(() => _transcript = 'Could not open history: $e');
    }
  }

  String _label(File file) {
    final micros = int.tryParse(file.path.split('/').last.split('.').first);
    return micros == null
        ? file.path.split('/').last
        : DateTime.fromMicrosecondsSinceEpoch(
            micros,
          ).toLocal().toString().split('.').first;
  }

  Future<void> _read(File file) async {
    setState(() {
      _selected = file;
      _transcript = 'Reading…';
    });
    final messages = <String, String>{};
    final names = <String, String>{};
    String goal = '';
    String phase = 'interrupted or closed';
    try {
      await for (final message in RoomArchive.read(file)) {
        if (message['type'] == 'snapshot') {
          messages.clear();
          for (final agent in message['roster'] as List? ?? []) {
            names[agent['id'] as String] = agent['handle'] as String;
          }
          goal =
              ((message['room'] as Map?)?['spec'] as Map?)?['goal']
                  as String? ??
              '';
        }
        if (message['type'] == 'phase') phase = message['phase'] as String;
        final event = (message['envelope'] as Map?)?['event'] as Map?;
        if (event?['event'] == 'message_posted') {
          final chat = event!['message'] as Map;
          final author = chat['author'] as Map;
          final name =
              author['name'] ??
              names[author['agent']] ??
              names[author['id']] ??
              author.toString();
          messages[chat['id'] as String] = '$name\n${chat['body']}';
        }
      }
      if (mounted && _selected == file) {
        setState(
          () => _transcript =
              'Goal: $goal\nLast recorded phase: $phase\n\n${messages.values.join('\n\n')}',
        );
      }
    } catch (e) {
      if (mounted) setState(() => _transcript = 'Could not read this run: $e');
    }
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text('${widget.roomName} · history')),
    body: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Padding(
          padding: EdgeInsets.all(16),
          child: Text(
            'Earlier saved sessions. Open the room to continue its conversation.',
          ),
        ),
        if (_files.isNotEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: DropdownButton<File>(
              isExpanded: true,
              value: _selected,
              items: [
                for (final file in _files)
                  DropdownMenuItem(value: file, child: Text(_label(file))),
              ],
              onChanged: (file) {
                if (file != null) _read(file);
              },
            ),
          ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: SelectableText(_transcript),
          ),
        ),
      ],
    ),
  );
}
