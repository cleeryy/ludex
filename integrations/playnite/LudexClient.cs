using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Threading;
using Newtonsoft.Json.Linq;

namespace Ludex.Playnite
{
    /// <summary>One game as returned by <c>GET /api/games</c>.</summary>
    public sealed class LudexGame
    {
        public long Id { get; set; }
        public string Title { get; set; }
        public string Description { get; set; }
        public string CoverUrl { get; set; }
        public string BackgroundUrl { get; set; }
        public DateTime? ReleaseDate { get; set; }
        public double? Rating { get; set; }
        public long MinutesPlayed { get; set; }
        public DateTime? LastPlayedAt { get; set; }
        public bool Installed { get; set; }
        public long TotalSize { get; set; }
        public int VersionCount { get; set; }
        public List<string> Genres { get; } = new List<string>();
        public List<string> Developers { get; } = new List<string>();
        public List<string> Publishers { get; } = new List<string>();
        public List<string> Tags { get; } = new List<string>();
    }

    /// <summary>
    /// Minimal read-only client for the Ludex JSON API.
    ///
    /// Only <c>GET /api/games</c> is used: the plugin never writes to the server
    /// and never downloads a game. Authentication accepts an API key with the
    /// <c>read</c> scope or a session JWT.
    /// </summary>
    public sealed class LudexClient : IDisposable
    {
        private const int PageSize = 200;
        private static readonly HttpClient Http = CreateClient();

        private readonly LudexConfig config;

        public LudexClient(LudexConfig config)
        {
            this.config = config ?? throw new ArgumentNullException(nameof(config));
        }

        private static HttpClient CreateClient()
        {
            var client = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
            client.DefaultRequestHeaders.UserAgent.ParseAdd("LudexPlaynite/0.1");
            client.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));
            return client;
        }

        public IEnumerable<LudexGame> GetGames(CancellationToken cancelToken)
        {
            var baseUrl = config.NormalizedServerUrl;
            if (string.IsNullOrEmpty(baseUrl))
            {
                throw new InvalidOperationException("Set ServerUrl in ludex.config.json.");
            }

            for (var page = 1; page <= 50; page++)
            {
                cancelToken.ThrowIfCancellationRequested();
                using (var response = Get($"{baseUrl}/api/games?limit={PageSize}&page={page}", cancelToken))
                {
                    response.EnsureSuccessStatusCode();
                    var text = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
                    var payload = JObject.Parse(text);
                    var items = payload["items"] as JArray;
                    if (items == null || items.Count == 0)
                    {
                        yield break;
                    }

                    foreach (var item in items)
                    {
                        yield return Map(item);
                    }

                    var total = payload.Value<long?>("total") ?? 0;
                    if (page * PageSize >= total)
                    {
                        yield break;
                    }
                }
            }
        }

        private HttpResponseMessage Get(string url, CancellationToken cancelToken)
        {
            var request = new HttpRequestMessage(HttpMethod.Get, url);
            if (!string.IsNullOrWhiteSpace(config.ApiKey))
            {
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", config.ApiKey.Trim());
            }
            return Http.SendAsync(request, cancelToken).GetAwaiter().GetResult();
        }

        private static LudexGame Map(JToken item)
        {
            var game = new LudexGame
            {
                Id = item.Value<long>("id"),
                Title = item.Value<string>("title"),
                Description = item.Value<string>("description"),
                CoverUrl = item.Value<string>("coverUrl"),
                BackgroundUrl = item.Value<string>("backgroundUrl"),
                ReleaseDate = ParseDate(item.Value<string>("releaseDate")),
                Rating = item.Value<double?>("rating"),
                MinutesPlayed = item.Value<long?>("minutesPlayed") ?? 0,
                LastPlayedAt = ParseDate(item.Value<string>("lastPlayedAt")),
                Installed = item.Value<bool?>("installed") ?? false,
                TotalSize = item.Value<long?>("totalSize") ?? 0,
                VersionCount = item.Value<int?>("versionCount") ?? 0,
            };

            CopyStrings(item["genres"], game.Genres);
            CopyStrings(item["developers"], game.Developers);
            CopyStrings(item["publishers"], game.Publishers);
            CopyStrings(item["tags"], game.Tags);
            return game;
        }

        private static void CopyStrings(JToken token, List<string> target)
        {
            var array = token as JArray;
            if (array == null)
            {
                return;
            }
            foreach (var value in array)
            {
                var text = value.Type == JTokenType.String ? value.Value<string>() : null;
                if (!string.IsNullOrWhiteSpace(text))
                {
                    target.Add(text);
                }
            }
        }

        private static DateTime? ParseDate(string value)
        {
            if (string.IsNullOrWhiteSpace(value))
            {
                return null;
            }
            if (DateTime.TryParse(value, null, System.Globalization.DateTimeStyles.RoundtripKind, out var parsed))
            {
                return parsed;
            }
            return null;
        }

        public void Dispose()
        {
            // The shared HttpClient lives for the process lifetime on purpose:
            // re-creating it per sync would exhaust sockets in Playnite.
        }
    }
}
