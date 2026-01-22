/*
    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        https://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <Util/URI.hpp>

namespace NES
{

struct DownloadResult
{
    std::filesystem::path localPath;
    size_t bytesDownloaded;
    std::optional<std::string> contentType;
};

class URLDownloader
{
public:
    /// Download URL content to target path
    /// @param url The URL to download from
    /// @param targetPath Local path to save the file
    /// @param timeoutSeconds Connection/read timeout
    /// @return DownloadResult on success
    /// @throws std::runtime_error on failure
    static DownloadResult downloadToFile(
        const URI& url,
        const std::filesystem::path& targetPath,
        int timeoutSeconds = 60);

    /// Check if a URL is reachable (HEAD request)
    static bool isReachable(const URI& url, int timeoutSeconds = 5);
};

} // namespace NES
