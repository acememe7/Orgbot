// Linux check that every native channel and plugin still answers when the
// Dart UI isolate runs on its own thread rather than GTK's main thread.
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:url_launcher/url_launcher.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const MaterialApp(home: Scaffold(body: SizedBox.expand())));
  await Future<void>.delayed(const Duration(seconds: 1));
  final results = <String, String>{};
  Future<void> check(String name, Future<Object?> Function() run) async {
    try {
      final value = await run().timeout(const Duration(seconds: 10));
      results[name] = 'ok ${value ?? ''}'.trim();
    } catch (error) {
      results[name] = 'FAILED $error';
    }
  }

  final dir = Directory.systemTemp.createTempSync('orgbot-channels-');
  File('${dir.path}/note.txt').writeAsStringSync('hello');
  const editor = MethodChannel('orgbot/workspace_editor');
  const panels = MethodChannel('orgbot/native_workspace_panels');
  await check('editor read', () async {
    final reply = await editor.invokeMethod<Object?>('read', {
      'root': dir.path,
      'path': 'note.txt',
    });
    return reply.toString().contains('hello') ? 'read file' : 'reply: $reply';
  });
  await check('browser panel', () async {
    await panels.invokeMethod<void>('browser_show', {
      'x': 40.0, 'y': 40.0, 'width': 400.0, 'height': 300.0,
      'uri': 'https://example.com',
    });
    await Future<void>.delayed(const Duration(milliseconds: 500));
    await panels.invokeMethod<void>('browser_hide');
    return 'shown and hidden';
  });
  await check('terminal panel', () async {
    await panels.invokeMethod<void>('terminal_show', {
      'x': 40.0, 'y': 40.0, 'width': 400.0, 'height': 300.0,
      'root': dir.path,
    });
    await Future<void>.delayed(const Duration(milliseconds: 500));
    await panels.invokeMethod<void>('terminal_stop');
    return 'started and stopped';
  });
  await check('shared preferences', () async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString('orgbot.channel.check', 'x');
    final value = prefs.getString('orgbot.channel.check');
    await prefs.remove('orgbot.channel.check');
    return value;
  });
  await check('secure storage', () async {
    const storage = FlutterSecureStorage();
    await storage.write(key: 'orgbot.channel.check', value: 'y');
    final value = await storage.read(key: 'orgbot.channel.check');
    await storage.delete(key: 'orgbot.channel.check');
    return value;
  });
  await check('url launcher', () => canLaunchUrl(Uri.parse('https://example.com')));
  dir.deleteSync(recursive: true);
  // ignore: avoid_print
  print('CHANNELS ${jsonEncode(results)}');
  exit(0);
}
