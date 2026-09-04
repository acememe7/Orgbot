// The sign-ins this device holds.
//
// A phone has no agent CLIs to sign in with, but the tokens those CLIs hold on
// a desktop are ordinary OAuth access tokens that the provider APIs accept
// directly. So the phone does not need its own OAuth client — it needs the
// sign-in you already have, carried across. `orgbot pair` on the desktop emits
// the bundle this reads.
//
// Tokens are secrets. They are stored on the device and never written into a
// room file, and `toString` on anything here redacts them, because these
// objects otherwise end up in logs and error messages.
//
// ignore_for_file: prefer_interpolation_to_compose_strings
library;

import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'model_catalog.dart';
import 'models.dart';

/// How a credential authenticates.
enum CredentialKind {
  /// An OAuth access token, sent as `Authorization: Bearer`.
  bearer,

  /// A long-lived API key.
  apiKey;

  static CredentialKind parse(String? raw) =>
      raw == 'api_key' ? CredentialKind.apiKey : CredentialKind.bearer;

  String get wire => this == CredentialKind.apiKey ? 'api_key' : 'bearer';
}

/// One provider's sign-in, plus the models it offers.
class ProviderCredential {
  ProviderCredential({
    required this.provider,
    required this.token,
    required this.kind,
    this.account,
    this.plan,
    this.models = const [],
    this.importedAt,
  });

  /// Which backend this signs in to.
  final ProviderKind provider;

  /// The secret. Never logged, never written into a room file.
  final String token;

  final CredentialKind kind;

  /// The account it belongs to, for display.
  final String? account;

  /// The plan or auth mode, for display — "pro", "chatgpt", and so on.
  final String? plan;

  /// Models this provider offers, carried over from the desktop's own cache
  /// so a phone with no CLI still shows a real list rather than a guess.
  final List<ModelOption> models;

  /// When this was imported.
  final DateTime? importedAt;

  Map<String, Object?> toJson() => {
        'provider': provider.key,
        'token': token,
        'kind': kind.wire,
        'account': account,
        'plan': plan,
        'models': [
          for (final model in models) {'id': model.id, 'label': model.label},
        ],
        'imported_at': (importedAt ?? DateTime.now()).millisecondsSinceEpoch,
      };

  static ProviderCredential? fromJson(Map<String, Object?> json) {
    final token = json['token'];
    if (token is! String || token.trim().isEmpty) return null;
    final provider = ProviderKind.fromKey(json['provider'] as String? ?? '');
    final rawModels = json['models'];
    return ProviderCredential(
      provider: provider,
      token: token,
      kind: CredentialKind.parse(json['kind'] as String?),
      account: json['account'] as String?,
      plan: json['plan'] as String?,
      models: rawModels is List
          ? rawModels
              .whereType<Map>()
              .map((m) => ModelOption(
                    (m['id'] ?? '').toString(),
                    (m['label'] ?? m['id'] ?? '').toString(),
                  ))
              .where((m) => m.id.isNotEmpty)
              .toList()
          : const [],
      importedAt: json['imported_at'] is num
          ? DateTime.fromMillisecondsSinceEpoch((json['imported_at'] as num).toInt())
          : null,
    );
  }

  /// Redacts the token: these objects reach logs and error messages.
  @override
  String toString() =>
      'ProviderCredential(${provider.key}, ${kind.wire}, <redacted>)';
}

/// What went wrong importing a bundle.
class PairingError implements Exception {
  PairingError(this.message);
  final String message;
  @override
  String toString() => message;
}

/// The bundle format this build understands.
const int kBundleVersion = 1;

/// The sign-ins held on this device.
class Credentials extends ChangeNotifier {
  // The field is filled lazily from SharedPreferences.getInstance() when it
  // was not injected, so it cannot be an initializing formal.
  // ignore: prefer_initializing_formals
  Credentials({SharedPreferences? prefs}) : _prefs = prefs;

  static const _key = 'orgbot_credentials_v1';

  SharedPreferences? _prefs;
  final Map<ProviderKind, ProviderCredential> _byProvider = {};

  /// Where the most recent bundle came from, for display.
  String? source;

  /// Every provider signed in here.
  List<ProviderCredential> get all => _byProvider.values.toList();

  /// The credential for a provider, if there is one.
  ProviderCredential? forProvider(ProviderKind provider) =>
      _byProvider[provider];

  /// Whether this provider can be used on this device.
  bool isSignedIn(ProviderKind provider) => _byProvider.containsKey(provider);

  /// Load what was stored previously.
  Future<void> load() async {
    final prefs = _prefs ??= await SharedPreferences.getInstance();
    final raw = prefs.getString(_key);
    if (raw == null || raw.isEmpty) return;
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! Map) return;
      source = decoded['source'] as String?;
      final providers = decoded['providers'];
      if (providers is List) {
        for (final entry in providers.whereType<Map>()) {
          final credential =
              ProviderCredential.fromJson(entry.cast<String, Object?>());
          if (credential != null) _byProvider[credential.provider] = credential;
        }
      }
      notifyListeners();
    } catch (_) {
      // Corrupt storage should not stop the app starting; the person can
      // simply pair again.
    }
  }

  /// Import a bundle produced by `orgbot pair`.
  ///
  /// Accepts the raw JSON, with or without surrounding whitespace, so pasting
  /// from a terminal works without trimming by hand.
  Future<int> importBundle(String text) async {
    final trimmed = text.trim();
    if (trimmed.isEmpty) {
      throw PairingError('Nothing to import.');
    }
    Object? decoded;
    try {
      decoded = jsonDecode(trimmed);
    } catch (_) {
      throw PairingError(
          'That is not a pairing bundle. Run `orgbot pair` and paste all of '
          'its output.');
    }
    if (decoded is! Map) {
      throw PairingError('That is not a pairing bundle.');
    }
    final version = decoded['version'];
    if (version is num && version > kBundleVersion) {
      throw PairingError(
          'This bundle was made by a newer OrgBot (v$version). Update the app.');
    }
    final providers = decoded['providers'];
    if (providers is! List || providers.isEmpty) {
      throw PairingError('That bundle has no sign-ins in it.');
    }

    var imported = 0;
    for (final entry in providers.whereType<Map>()) {
      final credential =
          ProviderCredential.fromJson(entry.cast<String, Object?>());
      if (credential == null) continue;
      _byProvider[credential.provider] = credential;
      imported++;
    }
    if (imported == 0) {
      throw PairingError('No usable sign-ins were found in that bundle.');
    }
    source = decoded['source'] as String?;
    await _persist();
    notifyListeners();
    return imported;
  }

  /// Forget one provider's sign-in.
  Future<void> forget(ProviderKind provider) async {
    _byProvider.remove(provider);
    await _persist();
    notifyListeners();
  }

  /// Forget everything.
  Future<void> forgetAll() async {
    _byProvider.clear();
    source = null;
    await _persist();
    notifyListeners();
  }

  Future<void> _persist() async {
    final prefs = _prefs ??= await SharedPreferences.getInstance();
    await prefs.setString(
      _key,
      jsonEncode({
        'version': kBundleVersion,
        'source': source,
        'providers': [for (final c in _byProvider.values) c.toJson()],
      }),
    );
  }
}
