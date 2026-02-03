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

#include <AdaptiveEngineStage.hpp>

#include <memory>
#include <sstream>
#include <Util/Logger/LogLevel.hpp>
#include <Util/Logger/Logger.hpp>
#include <gtest/gtest.h>
#include <BaseUnitTest.hpp>

namespace NES::AdaptiveEngine
{

class AdaptiveEngineStageTest : public ::testing::Test
{
protected:
    void SetUp() override
    {
        Logger::setupLogging("AdaptiveEngineStageTest.log", NES::LogLevel::LOG_DEBUG);
    }

    void TearDown() override {}
};

/// Test basic construction and destruction of AdaptiveEngineStage
TEST_F(AdaptiveEngineStageTest, BasicConstruction)
{
    auto stage = std::make_shared<AdaptiveEngineStage>("test-pipeline");

    EXPECT_EQ(stage->getPipelineId(), "test-pipeline");
    EXPECT_FALSE(stage->isRunning());
}

/// Test that stage can be moved
TEST_F(AdaptiveEngineStageTest, MoveConstruction)
{
    AdaptiveEngineStage stage1("test-pipeline");
    AdaptiveEngineStage stage2(std::move(stage1));

    EXPECT_EQ(stage2.getPipelineId(), "test-pipeline");
}

/// Test toString output
TEST_F(AdaptiveEngineStageTest, ToStringOutput)
{
    AdaptiveEngineStage stage("my-test-pipeline");

    std::ostringstream oss;
    oss << stage;

    std::string output = oss.str();
    EXPECT_TRUE(output.find("AdaptiveEngineStage") != std::string::npos);
    EXPECT_TRUE(output.find("my-test-pipeline") != std::string::npos);
}

} // namespace NES::AdaptiveEngine
