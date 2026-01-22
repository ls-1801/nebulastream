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

#include <Util/URLDownloader.hpp>

#include <fstream>
#include <stdexcept>

#define CPPHTTPLIB_OPENSSL_SUPPORT
#include <httplib.h>

#include <ErrorHandling.hpp>
#include <fmt/format.h>

namespace NES
{

DownloadResult URLDownloader::downloadToFile(const URI& url, const std::filesystem::path& targetPath, int timeoutSeconds)
{
    auto boostUrl = static_cast<boost::urls::url>(url);
    auto scheme = std::string(boostUrl.scheme());
    auto host = std::string(boostUrl.host());

    std::unique_ptr<httplib::Client> client;

    if (scheme == "https")
    {
        int port = boostUrl.port_number() ? boostUrl.port_number() : 443;
        client = std::make_unique<httplib::SSLClient>(host, port);
    }
    else if (scheme == "http")
    {
        int port = boostUrl.port_number() ? boostUrl.port_number() : 80;
        client = std::make_unique<httplib::Client>(host, port);
    }
    else
    {
        throw std::runtime_error(fmt::format("Unsupported URL scheme: {}", scheme));
    }

    client->set_connection_timeout(timeoutSeconds);
    client->set_read_timeout(timeoutSeconds);
    client->set_follow_location(true);

    std::string path = std::string(boostUrl.path());
    if (path.empty())
    {
        path = "/";
    }
    if (!boostUrl.query().empty())
    {
        path += "?" + std::string(boostUrl.query());
    }

    auto res = client->Get(path.c_str());

    if (!res)
    {
        throw std::runtime_error(fmt::format("Failed to download from URL: {} (error: {})", url.toString(), httplib::to_string(res.error())));
    }

    if (res->status != 200)
    {
        throw std::runtime_error(fmt::format("HTTP error {} while downloading from URL: {}", res->status, url.toString()));
    }

    std::filesystem::create_directories(targetPath.parent_path());

    std::ofstream ofs(targetPath, std::ios::binary);
    if (!ofs)
    {
        throw std::runtime_error(fmt::format("Failed to open file for writing: {}", targetPath.string()));
    }

    ofs.write(res->body.data(), static_cast<std::streamsize>(res->body.size()));

    if (!ofs)
    {
        throw std::runtime_error(fmt::format("Failed to write to file: {}", targetPath.string()));
    }

    std::optional<std::string> contentType;
    if (res->has_header("Content-Type"))
    {
        contentType = res->get_header_value("Content-Type");
    }

    return DownloadResult{
        .localPath = targetPath,
        .bytesDownloaded = res->body.size(),
        .contentType = contentType};
}

bool URLDownloader::isReachable(const URI& url, int timeoutSeconds)
{
    auto boostUrl = static_cast<boost::urls::url>(url);
    auto scheme = std::string(boostUrl.scheme());
    auto host = std::string(boostUrl.host());

    std::unique_ptr<httplib::Client> client;

    if (scheme == "https")
    {
        int port = boostUrl.port_number() ? boostUrl.port_number() : 443;
        client = std::make_unique<httplib::SSLClient>(host, port);
    }
    else if (scheme == "http")
    {
        int port = boostUrl.port_number() ? boostUrl.port_number() : 80;
        client = std::make_unique<httplib::Client>(host, port);
    }
    else
    {
        return false;
    }

    client->set_connection_timeout(timeoutSeconds);
    client->set_read_timeout(timeoutSeconds);
    client->set_follow_location(true);

    std::string path = std::string(boostUrl.path());
    if (path.empty())
    {
        path = "/";
    }
    if (!boostUrl.query().empty())
    {
        path += "?" + std::string(boostUrl.query());
    }

    auto res = client->Head(path.c_str());
    return res && (res->status >= 200 && res->status < 400);
}

} // namespace NES
