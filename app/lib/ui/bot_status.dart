import 'package:flutter/material.dart';

/// The colour and words for what a bot is doing, the same in every panel.
///
/// A bot the owner paused, the room benched, or that is offline says so,
/// whatever its last turn did. From the 13 November audit: a stale error
/// showed "last turn failed" over a pause, and the hosted roster showed
/// paused and benched bots as a green "idle".
(Color, String) botStatus(String doing, {bool lastTurnFailed = false}) {
  final settled = switch (doing) {
    'paused' => (Colors.grey, 'paused by you'),
    'benched' => (Colors.grey, 'benched: long turn, no task'),
    'offline' => (Colors.grey, 'offline'),
    'faulted' => (Colors.red, 'faulted'),
    _ => null,
  };
  if (settled != null) return settled;
  if (lastTurnFailed) return (Colors.red, 'last turn failed');
  return switch (doing) {
    'working' => (Colors.blue, 'working'),
    'reviewing' => (Colors.purple, 'reviewing'),
    'waiting' => (Colors.amber, 'waiting'),
    'between turns' => (Colors.blueGrey, 'between turns'),
    _ => (Colors.green, 'idle'),
  };
}
