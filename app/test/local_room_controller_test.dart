// The room controller a phone screen binds to: state transitions, live
// snapshots, and a background room run that finishes with a board and chat.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/engine/runner.dart';
import 'package:orgbot_app/state/local_room_controller.dart';

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

/// A tiny "model" that works the board by reading the situation.
class BoardBot implements BotProvider {
  final String handle;
  BoardBot(this.handle);
  @override
  String get name => handle;

  @override
  Future<BotTurn> act({required String situation}) async {
    final task = firstId(situation, 'task');
    if (handle == 'lead') {
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
    if (handle == 'writer') {
      if (state == 'ready') {
        return BotTurn(tools: [ToolCall('claim_task', {'task': task})]);
      }
      if (state == 'claimed') {
        await Future<void>.delayed(const Duration(milliseconds: 250));
        return BotTurn(tools: [
          ToolCall('start_task', {'task': task}),
          ToolCall('add_artifact', {'task': task, 'body': 'the work'}),
          ToolCall('submit_for_review', {'task': task}),
        ]);
      }
      if (state == 'approved') {
        return BotTurn(
            text: 'shipped',
            tools: [ToolCall('submit_task', {'task': task})]);
      }
      return const BotTurn();
    }
    if (handle == 'qa') {
      if (state == 'inReview') {
        return BotTurn(tools: [
          ToolCall('record_review', {'task': task, 'summary': 'good'})
        ]);
      }
      return const BotTurn();
    }
    return const BotTurn();
  }
}

void main() {
  test('a controller runs a room and reports the finished state', () async {
    final controller = LocalRoomController.demo('ship it',
        providerFor: (handle, roles) => BoardBot(handle));
    var notifies = 0;
    controller.addListener(() => notifies++);

    final result = await controller.start();
    expect(result, LocalRunState.finished);
    expect(controller.outcome, 'completed');

    final snap = controller.snapshot();
    expect(snap.tasks.single, contains('submitted'));
    expect(snap.chat, isNotEmpty);
    expect(notifies, greaterThan(0));
    expect(controller.state, LocalRunState.finished);
  });

  test('pausing mid-run surfaces as the paused state', () async {
    final controller = LocalRoomController.demo('ship it',
        providerFor: (handle, roles) => BoardBot(handle));
    // Let it begin, then pause before it can finish.
    final future = controller.start();
    await Future<void>.delayed(const Duration(milliseconds: 150));
    controller.pause();
    final state = await future;
    expect(state, LocalRunState.paused);
  });
}
