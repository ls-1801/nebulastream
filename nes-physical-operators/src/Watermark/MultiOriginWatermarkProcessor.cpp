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
#include <Watermark/MultiOriginWatermarkProcessor.hpp>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <sstream>
#include <string>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Sequencing/SequenceData.hpp>
#include <Time/Timestamp.hpp>
#include <fmt/ranges.h>
#include <ErrorHandling.hpp>

namespace NES
{

MultiOriginWatermarkProcessor::MultiOriginWatermarkProcessor(const std::vector<OriginId>& origins)
    : origins(origins), trackers(origins.size())
{
}

std::shared_ptr<MultiOriginWatermarkProcessor> MultiOriginWatermarkProcessor::create(const std::vector<OriginId>& origins)
{
    return std::make_shared<MultiOriginWatermarkProcessor>(origins);
}

Timestamp MultiOriginWatermarkProcessor::updateWatermark(Timestamp ts, SequenceData sequenceData, OriginId origin) const
{
    bool found = false;
    for (size_t originIndex = 0; originIndex < origins.size(); ++originIndex)
    {
        if (origins[originIndex] == origin)
        {
            trackers[originIndex].wlock()->insert(sequenceData.range, ts.getRawValue());
            found = true;
        }
    }
    INVARIANT(
        found,
        "update watermark for non existing origin={} number of origins size={} ids={}",
        origin,
        origins.size(),
        fmt::join(origins, ","));
    return getCurrentWatermark();
}

std::string MultiOriginWatermarkProcessor::getCurrentStatus()
{
    std::stringstream ss;
    for (size_t originIndex = 0; originIndex < origins.size(); ++originIndex)
    {
        auto locked = trackers[originIndex].rlock();
        auto val = locked->getCompletedValue();
        ss << " id=" << origins[originIndex] << " watermark=" << (val.has_value() ? val.value() : 0);
    }
    return ss.str();
}

Timestamp MultiOriginWatermarkProcessor::getCurrentWatermark() const
{
    uint64_t minimalWatermark = UINT64_MAX;
    for (const auto& tracker : trackers)
    {
        auto locked = tracker.rlock();
        auto val = locked->getCompletedValue();
        minimalWatermark = std::min(minimalWatermark, val.has_value() ? val.value() : uint64_t(0));
    }
    return Timestamp(minimalWatermark);
}

}
