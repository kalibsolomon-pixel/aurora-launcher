# Aurora Launcher

Aurora Launcher is a standalone desktop launcher for **Aurora**, a Fabric-based Minecraft: Java Edition client mod.

The launcher is currently in development. It is designed to install and launch isolated Aurora Minecraft instances while using the player's own Microsoft account and legitimate Minecraft: Java Edition entitlement.

## Purpose

Aurora Launcher provides a dedicated desktop interface for managing and launching Aurora. Its responsibilities include:

- authenticating a player through Microsoft OAuth;
- verifying Minecraft: Java Edition ownership before play;
- obtaining the player's Minecraft profile;
- installing official Minecraft files and metadata from Mojang services;
- installing the appropriate Fabric Loader version;
- installing a compatible Aurora release;
- managing isolated Minecraft instances and managed Java runtimes; and
- launching Minecraft with the authenticated player's Minecraft session.

Aurora Launcher does **not** provide Minecraft accounts, bypass Minecraft ownership requirements, or distribute authentication credentials.

## Microsoft and Minecraft authentication

Aurora Launcher uses the Microsoft OAuth 2.0 authorization-code flow with PKCE. Authentication takes place through Microsoft's authorization service; the launcher does not collect or store Microsoft account passwords.

After Microsoft authentication, the launcher uses the standard Xbox Live, XSTS, and Minecraft Services authentication flow required for third-party Minecraft: Java Edition launchers. Minecraft Services access is used to verify ownership, retrieve the authenticated player's Minecraft profile, and obtain the Minecraft access token required to launch the game.

The application requests Microsoft authorization for Xbox Live sign-in and offline access so a returning user can restore their session without repeatedly entering credentials. Persistent authentication credentials are intended to be stored using operating-system secure credential storage rather than plaintext launcher configuration files.

## Development status

The launcher's core installation, runtime-management, authentication, and launch pipeline has been implemented. End-to-end production authentication is pending approval of the launcher's Microsoft application for Minecraft Services access.

This repository is the public information and release location for Aurora Launcher. The launcher implementation and Aurora client source are currently maintained separately.

## Privacy and security

Aurora Launcher is designed around local operation and minimal data collection. It does not require an Aurora account and does not include unnecessary analytics or telemetry. Microsoft/Xbox/Minecraft credentials are used only for authentication and game-launch functionality.

Downloaded game and client artifacts are integrity-checked before activation, and Minecraft instances are kept isolated from the user's default `.minecraft` installation.

## Disclaimer

Aurora Launcher and Aurora are independent projects and are not affiliated with, endorsed by, or sponsored by Microsoft, Mojang Studios, or Fabric.

Minecraft is a trademark of Microsoft Corporation.