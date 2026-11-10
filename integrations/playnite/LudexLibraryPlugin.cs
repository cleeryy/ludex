using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using Playnite.SDK;
using Playnite.SDK.Models;
using Playnite.SDK.Plugins;

namespace Ludex.Playnite
{
    /// <summary>
    /// Read-only Playnite library plugin for a Ludex server (#14).
    ///
    /// It lists the server library and exposes every game as a Playnite entry
    /// whose Play action opens the matching Ludex URL. Nothing is downloaded and
    /// no game is modified on the server.
    /// </summary>
    public sealed class LudexLibraryPlugin : LibraryPlugin
    {
        // Stable id for this extension; Playnite keys installed libraries by it.
        public override Guid Id { get; } = Guid.Parse("8f1d6c7a-53b4-4f2e-9c1d-2b6a4e0d7c91");

        public override string Name => "Ludex";

        private readonly ILogger logger = LogManager.GetLogger();
        private readonly LudexConfig config;

        public LudexLibraryPlugin(IPlayniteAPI playniteApi) : base(playniteApi)
        {
            var directory = Path.GetDirectoryName(typeof(LudexLibraryPlugin).Assembly.Location)
                            ?? Directory.GetCurrentDirectory();
            config = LudexConfig.Load(directory);
        }

        public override IEnumerable<GameMetadata> GetGames(LibraryGetGamesArgs args)
        {
            var metadata = new List<GameMetadata>();
            try
            {
                using (var client = new LudexClient(config))
                {
                    foreach (var game in client.GetGames(args.CancelToken))
                    {
                        if (!config.IncludeUnavailable && game.VersionCount == 0)
                        {
                            continue;
                        }
                        metadata.Add(ToMetadata(game));
                    }
                }
            }
            catch (Exception error)
            {
                // A failed sync must never crash Playnite: log and return what we
                // have, which is nothing on the first run.
                logger.Error(error, "Ludex library sync failed");
                PlayniteApi.Notifications.Add(new NotificationMessage(
                    "ludex-sync-failed",
                    $"Ludex: could not read the library ({error.Message}). Check ludex.config.json.",
                    NotificationType.Error));
            }

            return metadata;
        }

        private GameMetadata ToMetadata(LudexGame game)
        {
            var launchUrl = config.BuildLaunchUrl(game.Id);
            var metadata = new GameMetadata
            {
                Name = game.Title,
                GameId = game.Id.ToString(System.Globalization.CultureInfo.InvariantCulture),
                Description = game.Description,
                ReleaseDate = game.ReleaseDate,
                IsInstalled = game.Installed,
                Source = new MetadataNameProperty("Ludex"),
                Playtime = (ulong?)Math.Max(0, game.MinutesPlayed),
                LastActivity = game.LastPlayedAt,
                Links = new List<Link>
                {
                    new Link("Ludex", $"{config.NormalizedServerUrl}/game/{game.Id}"),
                },
                GameActions = new List<GameAction>
                {
                    new GameAction
                    {
                        Name = "Play",
                        Type = GameActionType.URL,
                        Path = launchUrl,
                        IsPlayAction = true,
                    },
                },
            };

            if (!string.IsNullOrWhiteSpace(game.CoverUrl))
            {
                metadata.CoverImage = new MetadataFile(game.CoverUrl);
            }
            if (!string.IsNullOrWhiteSpace(game.BackgroundUrl))
            {
                metadata.BackgroundImage = new MetadataFile(game.BackgroundUrl);
            }
            if (game.Developers.Count > 0)
            {
                metadata.Developers = new HashSet<string>(game.Developers);
            }
            if (game.Publishers.Count > 0)
            {
                metadata.Publishers = new HashSet<string>(game.Publishers);
            }
            if (game.Genres.Count > 0)
            {
                metadata.Genres = new HashSet<MetadataNameProperty>(
                    game.Genres.Select(genre => new MetadataNameProperty(genre)));
            }
            if (game.Tags.Count > 0)
            {
                metadata.Tags = new HashSet<MetadataNameProperty>(
                    game.Tags.Select(tag => new MetadataNameProperty(tag)));
            }

            return metadata;
        }
    }
}
