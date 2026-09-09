import 'dart:io';
import 'package:flutter/services.dart';

/// Android's foreground service keeps the cached Flutter engine and SSH alive.
class BackgroundRooms {
  static const channel = MethodChannel('orgbot/background');

  static Future<void> update(String id, bool active) async {
    if (Platform.isAndroid) {
      await channel.invokeMethod<void>('room', {'id': id, 'active': active});
    }
  }
}
