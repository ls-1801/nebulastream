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

#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>

namespace NES
{

class URLDownloaderTest : public ::testing::Test
{
protected:
    void SetUp() override
    {
        tempDir = std::filesystem::temp_directory_path() / "nes_url_test";
        std::filesystem::create_directories(tempDir);
    }

    void TearDown() override
    {
        std::filesystem::remove_all(tempDir);
    }

    std::filesystem::path tempDir;
};

TEST_F(URLDownloaderTest, DownloadFromTubCloud)
{
    const std::string testUrl =
        "https://tubcloud.tu-berlin.de/s/28Tr2wTd73Ggeed/download"
        "?files=MD5_d7e6113eb67d79644947ac6bc32a80bf";

    auto targetPath = tempDir / "test_data.bin";

    auto result = URLDownloader::downloadToFile(URI(testUrl), targetPath);

    EXPECT_TRUE(std::filesystem::exists(result.localPath));
    EXPECT_GT(result.bytesDownloaded, 0);
    EXPECT_EQ(result.localPath, targetPath);
}

TEST_F(URLDownloaderTest, IsReachable)
{
    const std::string reachableUrl = "https://www.google.com";
    EXPECT_TRUE(URLDownloader::isReachable(URI(reachableUrl)));

    const std::string unreachableUrl = "https://this-domain-does-not-exist-12345.com";
    EXPECT_FALSE(URLDownloader::isReachable(URI(unreachableUrl)));
}

TEST_F(URLDownloaderTest, DownloadCreatesParentDirectories)
{
    const std::string testUrl =
        "https://tubcloud.tu-berlin.de/s/28Tr2wTd73Ggeed/download"
        "?files=MD5_d7e6113eb67d79644947ac6bc32a80bf";

    auto targetPath = tempDir / "nested" / "dir" / "test_data.bin";

    auto result = URLDownloader::downloadToFile(URI(testUrl), targetPath);

    EXPECT_TRUE(std::filesystem::exists(result.localPath));
    EXPECT_TRUE(std::filesystem::exists(tempDir / "nested" / "dir"));
}

TEST_F(URLDownloaderTest, DownloadFailsForInvalidURL)
{
    const std::string invalidUrl = "https://this-domain-does-not-exist-12345.com/file.bin";
    auto targetPath = tempDir / "should_not_exist.bin";

    EXPECT_THROW(URLDownloader::downloadToFile(URI(invalidUrl), targetPath), std::runtime_error);
    EXPECT_FALSE(std::filesystem::exists(targetPath));
}

} // namespace NES
