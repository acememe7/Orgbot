// ignore_for_file: prefer_interpolation_to_compose_strings

// A keyless scripted bot that works the board by reading the situation - the
// phone's stand-in for a model, and how new users watch rooms move before
// adding API keys.
library;

import '../engine/runner.dart';

class ScriptedRoomBot implements BotProvider {
  final String handle;
  final String role; // writer | coordinator | reviewer
  ScriptedRoomBot(this.handle, this.role);

  @override
  String get name => handle;

  String firstId(String text, String prefix) {
    final words = text.split(RegExp('[^A-Za-z0-9_]+'));
    for (final w in words) {
      if (w.startsWith(prefix + '_') && w.length > prefix.length + 2) return w;
    }
    return '';
  }

  String boardState(String text, String id) {
    final i = text.lastIndexOf('Board:');
    final board = i < 0 ? text : text.substring(i);
    final j = board.lastIndexOf(id);
    if (j < 0) return '';
    final before = board.substring(0, j).trim();
    final words = before.split(RegExp('\\s+'));
    return words.isEmpty ? '' : words.last;
  }

  @override
  Future<BotTurn> act({required String situation}) async {
    final task = firstId(situation, 'task');
    if (role == 'coordinator') {
      if (task.isEmpty) {
        return const BotTurn(tools: [
          ToolCall('create_task', {
            'title': 'do the job',
            'brief': 'claim, work, review, ship',
            'required_role': 'writer',
          })
        ]);
      }
      return const BotTurn();
    }
    if (task.isEmpty) return const BotTurn();
    final state = boardState(situation, task);
    if (role == 'reviewer') {
      if (state == 'inReview') {
        return BotTurn(tools: [
          ToolCall('record_review', {'task': task, 'summary': 'looks good'})
        ]);
      }
      return const BotTurn();
    }
    // writer
    if (state == 'ready') {
      return BotTurn(tools: [ToolCall('claim_task', {'task': task})]);
    }
    if (state == 'claimed') {
      return BotTurn(tools: [
        ToolCall('start_task', {'task': task}),
        ToolCall('add_artifact', {'task': task, 'body': 'the work'}),
        ToolCall('submit_for_review', {'task': task}),
      ]);
    }
    if (state == 'approved') {
      return BotTurn(
          text: 'shipped it',
          tools: [ToolCall('submit_task', {'task': task})]);
    }
    return const BotTurn();
  }
}
