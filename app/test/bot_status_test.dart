import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:orgbot_app/protocol/models.dart';
import 'package:orgbot_app/ui/bot_status.dart';

void main() {
  // From the 13 November audit: a stale error hid a pause, the hosted roster
  // showed benched bots as idle, and the current turn's time never moved.
  test('paused, benched and offline win over a stale error', () {
    expect(botStatus('paused', lastTurnFailed: true).$2, 'paused by you');
    expect(botStatus('benched').$2, startsWith('benched'));
    expect(botStatus('offline', lastTurnFailed: true).$1, Colors.grey);
    expect(botStatus('idle', lastTurnFailed: true).$2, 'last turn failed');
    expect(botStatus('idle').$2, 'idle');
  });

  test('a turn under way counts up from when it was read', () {
    final time = BotTime(
      inTurn: true,
      currentTurnMs: 0,
      receivedAt: DateTime.now().subtract(const Duration(minutes: 3)),
    );
    expect(time.isEmpty, isFalse);
    expect(time.summary, startsWith('This turn 3 min'));
    expect(BotTime.fromJson({'in_turn': true, 'current_turn_ms': 5000}).summary, startsWith('This turn 5 s'));
  });
}
